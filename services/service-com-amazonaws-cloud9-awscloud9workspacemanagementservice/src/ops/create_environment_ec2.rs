// SPDX-License-Identifier: CC0-1.0

//! @op CreateEnvironmentEC2
//!
//! Creates a Cloud9 EC2 development environment: mints a fresh `environmentId`,
//! persists the whole record in [`crate::state`], and reports the id. Every
//! later `Describe*`/`List*`/`Update*`/`Delete*` answer is reconstructed from
//! that record.
//!
//! Reaches this service's store through [`crate::state::service_state`] with the
//! engine held on `self`; it never constructs an engine.
//!
//! ---
//!
//! # Modeled contract for `CreateEnvironmentEC2`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloud9-awscloud9workspacemanagementservice/smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json")
//! ```
//!
//! Creates an Cloud9 development environment, launches an Amazon Elastic Compute Cloud
//! (Amazon EC2) instance, and then connects from the instance to the environment. Cloud9 is
//! no longer available to new customers. Existing customers of Cloud9 can continue to use
//! the service as normal. Learn more"
//!
//! ## Input
//!
//! * `automaticStopTimeMinutes` (AutomaticStopTimeMinutes, range 0..20160) — The number of
//!   minutes until the running instance is shut down after the environment has last been
//!   used.
//! * `clientRequestToken` (ClientRequestToken, pattern ^[\x20-\x7E]{10,128}$) — A unique,
//!   case-sensitive string that helps Cloud9 to ensure this operation completes no more than
//!   one time. For more information, see Client Tokens in the Amazon EC2 API Reference.
//! * `connectionType` (ConnectionType, one of: CONNECT_SSH | CONNECT_SSM) — The connection
//!   type used for connecting to an Amazon EC2 environment. Valid values are CONNECT_SSH
//!   (default) and CONNECT_SSM (connected through Amazon EC2 Systems Manager). For more
//!   information, see Accessing no-ingress EC2 instances with Amazon EC2 Systems Manager in
//!   the Cloud9 User Guide.
//! * `description` (EnvironmentDescription, len 0..200) — The description of the environment
//!   to create.
//! * `dryRun` (NullableBoolean) — Checks whether you have the required permissions for the
//!   action, without actually making the request, and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `imageId` (ImageId, len 0..512) **required** — The identifier for the Amazon Machine
//!   Image (AMI) that's used to create the EC2 instance. To choose an AMI for the instance,
//!   you must specify a valid AMI alias or a valid Amazon EC2 Systems Manager (SSM) path. We
//!   recommend using Amazon Linux 2023 as the AMI to create your environment as it is fully
//!   supported. From December 16, 2024, Ubuntu 18.04 will be removed from the list of
//!   available imageIds for Cloud9. This change is necessary as Ubuntu 18.04 has ended
//!   standard support on May 31, 2023. This change will only affect direct API consumers, and
//!   not Cloud9 console users. Since Ubuntu 18.04 has ended standard support as of May 31,
//!   2023, we recommend you choose Ubuntu 22.04. AMI aliases …
//! * `instanceType` (InstanceType, len 5..20, pattern ^[a-z]+[1-9][.][a-z0-9]+$) **required**
//!   — The type of instance to connect to the environment (for example, t2.micro).
//! * `name` (EnvironmentName, len 1..60) **required** — The name of the environment to
//!   create. This name is visible to other IAM users in the same Amazon Web Services account.
//! * `ownerArn` (UserArn, pattern
//!   ^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):(iam|sts)::\d+:(root|(user\/[\w+=/:,.@-]{1,64}|federated-user\/[\w+=/:,.@
//!   …) — The Amazon Resource Name (ARN) of the environment owner. This ARN can be the ARN of
//!   any IAM principal. If this value is not specified, the ARN defaults to this
//!   environment's creator.
//! * `subnetId` (SubnetId, len 15..24, pattern ^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$) —
//!   The ID of the subnet in Amazon VPC that Cloud9 will use to communicate with the Amazon
//!   EC2 instance.
//! * `tags` (TagList, len 0..200) — An array of key-value pairs that will be associated with
//!   the new Cloud9 development environment.
//!
//! ## Output
//!
//! * `environmentId` (EnvironmentId, pattern ^[a-zA-Z0-9]{8,32}$) — The ID of the environment
//!   that was created.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The target request is invalid.
//! * `ConflictException` (HTTP 400) — A conflict occurred.
//! * `ForbiddenException` (HTTP 400) — An access permissions issue occurred.
//! * `InternalServerErrorException` (HTTP 500) — An internal server error occurred.
//! * `LimitExceededException` (HTTP 400) — A service limit was exceeded.
//! * `NotFoundException` (HTTP 400) — The target resource cannot be found.
//! * `TooManyRequestsException` (HTTP 400) — Too many service requests were made over the
//!   given time period.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{
    ConnectionType, Environment as EnvironmentRecord, EnvironmentLifecycleState,
    EnvironmentLifecycleStatus, EnvironmentMember as EnvironmentMemberRecord, EnvironmentStatus,
    EnvironmentType, ManagedCredentialsStatus, Permissions, CREATION_SETTLES_AFTER,
};
use std::collections::BTreeMap;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEnvironmentEC2Error` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEnvironmentEC2Error` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloud9#CreateEnvironmentEC2")]
#[derive(Debug)]
pub enum CreateEnvironmentEC2ErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_environment_ec2::CreateEnvironmentEC2Error`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEnvironmentEC2Error = crate::scaffolded::Scaffolded<CreateEnvironmentEC2ErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEnvironmentEC2ErrorModeled> for CreateEnvironmentEC2Error {
    fn from(modeled: CreateEnvironmentEC2ErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `BadRequestException` — "The target request is invalid." `code` keeps the
/// model's declared default and `className` is left unset: the model gives
/// neither a value nor a rule for them.
fn bad_request(message: String) -> CreateEnvironmentEC2Error {
    CreateEnvironmentEC2ErrorModeled::BadRequestException(BadRequestException {
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// The `CreateEnvironmentEC2` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEnvironmentEC2<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEnvironmentEC2<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEnvironmentEC2<Eng>
{
    type Input = CreateEnvironmentEC2Input;
    type Output = CreateEnvironmentEC2Output;
    type Error = CreateEnvironmentEC2Error;

    const NAME: &'static str = "CreateEnvironmentEC2";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_EC2_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_EC2_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateEnvironmentEC2ErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the Cloud9 environment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        // The model documents CONNECT_SSH as `connectionType`'s default. A value
        // outside the enum is refused at the boundary, so reaching here with an
        // unknown one would mean the model changed under us — refuse it as the
        // invalid request it is rather than silently picking a connection type.
        let connection_type = match input.connection_type.as_deref() {
            None => ConnectionType::ConnectSsh,
            Some(value) => ConnectionType::from_wire(value).ok_or_else(|| {
                bad_request(format!(
                    "connectionType '{value}' is not a supported connection type"
                ))
            })?,
        };

        // "If this value is not specified, the ARN defaults to this
        // environment's creator." A supplied one must name an IAM or STS
        // principal, which is what the modeled `UserArn` accepts.
        if let Some(supplied) = input.owner_arn.as_deref() {
            if !crate::state::is_principal_arn(supplied) {
                return Err(bad_request(format!(
                    "ownerArn '{supplied}' is not the ARN of an IAM principal"
                )));
            }
        }
        let owner_arn = input
            .owner_arn
            .clone()
            .unwrap_or_else(|| crate::state::caller_user_arn(ctx.parts()));
        let region = crate::state::request_region(ctx.parts());

        let tags: BTreeMap<String, String> = input
            .tags
            .iter()
            .flatten()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect();

        // A dry run validates and persists NOTHING, and is refused BEFORE the
        // idempotency replay below: a dry run must never answer with an id, not
        // even one an earlier create with the same token already minted. The
        // model documents an EC2-style dry run: "provides an error response. If
        // you have the required permissions, the error response is
        // DryRunOperation." This service models no `DryRunOperation` shape and
        // the emulator does not invent error codes, so the refusal carries the
        // client-input error it DOES model, `BadRequestException`, with the
        // message that says the request would have succeeded.
        if input.dry_run == Some(true) {
            return Err(bad_request(
                "Request would have succeeded, but DryRun flag is set".to_string(),
            ));
        }

        let mut store = state.lock();

        // `@idempotent`: the same `clientRequestToken` answers with the id it
        // created the first time instead of creating a second environment.
        if let Some(token) = input.client_request_token.as_deref() {
            if let Some(existing) = store.idempotency.get(token) {
                if store.environments.contains_key(existing) {
                    return Ok(CreateEnvironmentEC2Output {
                        environment_id: Some(existing.clone()),
                    });
                }
            }
        }

        // The store is BOUNDED: past the cap a create is refused with the error
        // AWS gives when an account's environment quota is reached, so a caller
        // cannot grow the process's memory without limit. This also bounds
        // `idempotency`, which never holds more than one token per resident
        // environment.
        if store.is_at_environment_quota() {
            return Err(CreateEnvironmentEC2ErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "this account already holds the maximum of {} Cloud9 environments",
                        crate::state::ENVIRONMENTS_PER_ACCOUNT
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let environment_id = store.mint_environment_id();
        let created_at = SystemTime::now();
        let owner_user_id = crate::state::principal_unique_id(&owner_arn);
        let environment = EnvironmentRecord {
            id: environment_id.clone(),
            name: input.name,
            description: input.description,
            // `CreateEnvironmentEC2` is the EC2 flavour of the resource, so the
            // `@required` `type` member is `ec2` for everything it creates.
            env_type: EnvironmentType::Ec2,
            connection_type,
            arn: crate::state::environment_arn(&region, &environment_id),
            owner_arn: owner_arn.clone(),
            instance_type: input.instance_type,
            image_id: input.image_id,
            subnet_id: input.subnet_id,
            automatic_stop_time_minutes: input.automatic_stop_time_minutes,
            // Creation is not instantaneous: the environment is reported
            // `creating` until the clock makes the transition due.
            status: EnvironmentStatus::Creating,
            status_message: "Environment is being created".to_string(),
            lifecycle: EnvironmentLifecycleState {
                status: EnvironmentLifecycleStatus::Creating,
                reason: None,
                failure_resource: None,
            },
            // The model's own value for credentials enabled as part of creating
            // the environment.
            managed_credentials_status: Some(ManagedCredentialsStatus::EnabledOnCreate),
            created_at,
            settles_at: created_at + CREATION_SETTLES_AFTER,
            // The creator is the environment's single `owner` member.
            members: vec![EnvironmentMemberRecord {
                environment_id: environment_id.clone(),
                user_arn: owner_arn,
                user_id: owner_user_id,
                permissions: Permissions::Owner,
                last_access: None,
            }],
            tags,
        };

        store
            .environments
            .insert(environment_id.clone(), environment);
        if let Some(token) = input.client_request_token {
            store.idempotency.insert(token, environment_id.clone());
        }

        Ok(CreateEnvironmentEC2Output {
            environment_id: Some(environment_id),
        })
    }
}
