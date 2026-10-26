// SPDX-License-Identifier: CC0-1.0

//! @op GetApplication
//!
//! Reports a persisted application: its name, ARN, status, latest version, engine type,
//! creation time and everything `CreateApplication`/`UpdateApplication` recorded. An id
//! no application answers to is a 404 `ResourceNotFoundException`.
//!
//! This is a REPORTING operation (`@readonly` in the model): it writes nothing. The
//! `Creating` -> `Created` status transition the clock owes is COMPUTED from the stored
//! settle instant (see [`crate::state::CREATION_SETTLES_AFTER`]), so reading an
//! application never changes what a later caller sees.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json")
//! ```
//!
//! Describes the details of a specific application.
//!
//! ## Input
//!
//! * `applicationId` (Identifier, pattern ^\S{1,80}$) **required** — The identifier of the
//!   application.
//!
//! ## Output
//!
//! * `applicationArn` (Arn, pattern
//!   ^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:([a-z]{2}-((iso[a-z]{0,1}-)|(gov-
//!   …) **required** — The Amazon Resource Name (ARN) of the application.
//! * `applicationId` (Identifier, pattern ^\S{1,80}$) **required** — The identifier of the
//!   application.
//! * `creationTime` (Timestamp) **required** — The timestamp when this application was
//!   created.
//! * `deployedVersion` (DeployedVersionSummary) — The version of the application that is
//!   deployed.
//! * `description` (EntityDescription, len 0..500) — The description of the application.
//! * `engineType` (EngineType, one of: bluage | microfocus) **required** — The type of the
//!   target platform for the application.
//! * `environmentId` (Identifier, pattern ^\S{1,80}$) — The identifier of the runtime
//!   environment where you want to deploy the application.
//! * `kmsKeyId` (String) — The identifier of a customer managed key.
//! * `lastStartTime` (Timestamp) — The timestamp when you last started the application. Null
//!   until the application runs for the first time.
//! * `latestVersion` (ApplicationVersionSummary) **required** — The latest version of the
//!   application.
//! * `listenerArns` (ArnList, len 1..) — The Amazon Resource Name (ARN) for the network load
//!   balancer listener created in your Amazon Web Services account. Amazon Web Services
//!   Mainframe Modernization creates this listener for you the first time you deploy an
//!   application.
//! * `listenerPorts` (PortList, len 1..) — The port associated with the network load balancer
//!   listener created in your Amazon Web Services account.
//! * `loadBalancerDnsName` (String100, pattern ^\S{1,100}$) — The public DNS name of the load
//!   balancer created in your Amazon Web Services account.
//! * `logGroups` (LogGroupSummaries) — The list of log summaries. Each log summary includes
//!   the log type as well as the log group identifier. These are CloudWatch logs. Amazon Web
//!   Services Mainframe Modernization pushes the application log to CloudWatch under the
//!   customer's account.
//! * `name` (EntityName, pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$) **required** — The unique
//!   identifier of the application.
//! * `roleArn` (Arn, pattern
//!   ^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:([a-z]{2}-((iso[a-z]{0,1}-)|(gov-
//!   …) — The Amazon Resource Name (ARN) of the role associated with the application.
//! * `status` (ApplicationLifecycle, one of: Available | Created | Creating | Deleting |
//!   Deleting From Environment | Failed | Ready | Running | Starting | Stopped | Stopping)
//!   **required** — The status of the application.
//! * `statusReason` (String) — The reason for the reported status.
//! * `tags` (TagMap, len 0..200) — A list of tags associated with the application.
//! * `targetGroupArns` (ArnList, len 1..) — Returns the Amazon Resource Names (ARNs) of the
//!   target groups that are attached to the network load balancer.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The account or role doesn't have the right
//!   permissions to make the request.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found.
//! * `ThrottlingException` (HTTP 429) — The number of requests made exceeds the limit.
//! * `ValidationException` (HTTP 400) — One or more parameters provided in the request is not
//!   valid.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.m2#GetApplication")]
#[derive(Debug)]
pub enum GetApplicationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_application::GetApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetApplicationError = crate::scaffolded::Scaffolded<GetApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetApplicationErrorModeled> for GetApplicationError {
    fn from(modeled: GetApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetApplication<Eng>
{
    type Input = GetApplicationInput;
    type Output = GetApplicationOutput;
    type Error = GetApplicationError;

    const NAME: &'static str = "GetApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetApplicationErrorModeled::InternalServerException(InternalServerException {
                message: "the m2 application store is unavailable".to_string(),
                retry_after_seconds: 0,
            })
        })?;

        let now = SystemTime::now();
        let store = state.lock();
        let application = store.applications.get(&input.application_id).ok_or_else(|| {
            GetApplicationErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: crate::state::application_not_found_message(&input.application_id),
                resource_id: Some(input.application_id.clone()),
                resource_type: Some("application".to_string()),
            })
        })?;

        let latest = application.latest().ok_or_else(|| {
            GetApplicationErrorModeled::InternalServerException(InternalServerException {
                message: format!(
                    "application {} has no recorded version",
                    application.application_id
                ),
                retry_after_seconds: 0,
            })
        })?;
        let tags = store.tags_for(&application.application_arn);

        Ok(GetApplicationOutput {
            name: application.name.clone(),
            description: application.description.clone(),
            application_id: application.application_id.clone(),
            application_arn: application.application_arn.clone(),
            status: application.status(now).to_string(),
            latest_version: ApplicationVersionSummary {
                application_version: latest.application_version,
                status: latest.status(now).to_string(),
                status_reason: latest.status_reason.clone(),
                creation_time: ::aws_smithy_types::DateTime::from(latest.creation_time),
            },
            // A deployed version, log groups, load balancer and listeners only exist
            // once the application has been deployed to a runtime environment, which
            // this slice of the service does not record.
            deployed_version: None,
            engine_type: application.engine_type.clone(),
            log_groups: None,
            creation_time: ::aws_smithy_types::DateTime::from(application.creation_time),
            last_start_time: application
                .last_start_time
                .map(::aws_smithy_types::DateTime::from),
            tags: (!tags.is_empty()).then_some(tags),
            environment_id: application.environment_id.clone(),
            target_group_arns: None,
            listener_arns: None,
            listener_ports: None,
            load_balancer_dns_name: None,
            status_reason: None,
            kms_key_id: application.kms_key_id.clone(),
            role_arn: application.role_arn.clone(),
        })
    }
}
