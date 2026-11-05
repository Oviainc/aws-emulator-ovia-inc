// SPDX-License-Identifier: CC0-1.0

//! @op CreateApplicationVersion
//! Appends one version to an application already in [`crate::state`], keyed by the
//! `SemanticVersion` path label, and answers from the version it stored — so a later
//! ListApplicationVersions/GetApplication sees exactly this version.
//!
//! A version carries "the raw packaged AWS SAM template of your application", so a
//! template body this emulator reads is published only if it declares the SAM `Transform`
//! field; one that does not is refused with the modelled `BadRequestException` and
//! nothing is stored for it (see [`crate::state::parse_template`]).
//!
//! This is the ONLY file to touch for this operation.
//!
//! The engine is held on `self` (constructor-injected); reach the store through
//! [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateApplicationVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Creates an application version.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `SemanticVersion` (__string) **required** — The semantic version of the new version.
//! * `SourceCodeArchiveUrl` (__string) — A link to the S3 object that contains the ZIP
//!   archive of the source code for this version of your application. Maximum size 50 MB
//! * `SourceCodeUrl` (__string) — A link to a public repository for the source code of your
//!   application, for example the URL of a specific GitHub commit.
//! * `TemplateBody` (__string) — The raw packaged AWS SAM template of your application.
//! * `TemplateUrl` (__string) — A link to the packaged AWS SAM template of your application.
//!
//! ## Output
//!
//! * `ApplicationId` (__string) — The application Amazon Resource Name (ARN).
//! * `CreationTime` (__string) — The date and time this resource was created.
//! * `ParameterDefinitions` (__listOfParameterDefinition) — An array of parameter types
//!   supported by the application.
//! * `RequiredCapabilities` (__listOfCapability) — A list of values that you must specify
//!   before you can deploy certain applications. Some applications might include resources
//!   that can affect permissions in your AWS account, for example, by creating new AWS
//!   Identity and Access Management (IAM) users. For those applications, you must explicitly
//!   acknowledge their capabilities by specifying this parameter. The only valid values are
//!   CAPABILITY_IAM, CAPABILITY_NAMED_IAM, CAPABILITY_RESOURCE_POLICY, and
//!   CAPABILITY_AUTO_EXPAND. The following resources require you to specify CAPABILITY_IAM or
//!   CAPABILITY_NAMED_IAM: AWS::IAM::Group, AWS::IAM::InstanceProfile, AWS::IAM::Policy, and
//!   AWS::IAM::Role. If the application contains IAM resources, you can …
//! * `ResourcesSupported` (__boolean) — Whether all of the AWS resources contained in this
//!   application are supported in the region in which it is being retrieved.
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//! * `SourceCodeArchiveUrl` (__string) — A link to the S3 object that contains the ZIP
//!   archive of the source code for this version of your application. Maximum size 50 MB
//! * `SourceCodeUrl` (__string) — A link to a public repository for the source code of your
//!   application, for example the URL of a specific GitHub commit.
//! * `TemplateUrl` (__string) — A link to the packaged AWS SAM template of your application.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — One of the parameters in the request is invalid.
//! * `ConflictException` (HTTP 409) — The resource already exists.
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{AppVersion, Artifact};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateApplicationVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateApplicationVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `extra_errors` widens the set with `NotFoundException`, a shape this SERVICE models
// (and GetApplication/DeleteApplication list) that this operation's own `errors` list
// omits: a version can only be added to an application that exists, and an
// `ApplicationId` naming no application is an absent resource — HTTP 404 — not an
// invalid parameter. The model's error list for this operation is the only place that
// says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.serverlessapplicationrepository#CreateApplicationVersion",
    extra_errors = ["NotFoundException"]
)]
#[derive(Debug)]
pub enum CreateApplicationVersionErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_application_version::CreateApplicationVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateApplicationVersionError = crate::scaffolded::Scaffolded<CreateApplicationVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateApplicationVersionErrorModeled> for CreateApplicationVersionError {
    fn from(modeled: CreateApplicationVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateApplicationVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateApplicationVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateApplicationVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateApplicationVersion<Eng>
{
    type Input = CreateApplicationVersionInput;
    type Output = CreateApplicationVersionOutput;
    type Error = CreateApplicationVersionError;

    const NAME: &'static str = "CreateApplicationVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APPLICATION_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APPLICATION_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateApplicationVersionErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        // "You can specify only one of templateBody and templateUrl; otherwise an error
        // results" — a sentence the model states in prose, so the service enforces it.
        if input.template_body.is_some() && input.template_url.is_some() {
            let (body_member, url_member) = Artifact::Template.members();
            return Err(bad_request(format!(
                "You can specify only one of {body_member} and {url_member}."
            )));
        }

        let region = crate::state::request_region(ctx);
        // The endpoint this request arrived on, which the locator minted for an inline
        // template body announces; see `crate::state::minted_artifact_url`.
        let host = crate::state::request_host(ctx, &region);
        // The semantic version is the key this version is stored under and is echoed
        // back, so it must be one: "The semantic version of the new version" per
        // https://semver.org/. The refusal is the service's fixed sentence for the
        // version grammar ([`crate::state::INVALID_SEMANTIC_VERSION_MESSAGE`]), which
        // carries nothing of the request.
        if !crate::state::is_semantic_version(&input.semantic_version) {
            return Err(bad_request(crate::state::INVALID_SEMANTIC_VERSION_MESSAGE));
        }
        // The inline template is retained for the life of the version, so its size is
        // bounded before it is parsed and before it is stored, at the documented maximum
        // of a packaged CloudFormation template.
        crate::state::bounded_bytes(
            "TemplateBody",
            input.template_body.as_ref(),
            crate::state::MAX_TEMPLATE_BYTES,
        )
        .map_err(bad_request)?;
        // Read BEFORE the store is locked, and bounded against the documented template
        // maxima: the template is caller-supplied, so neither the work it costs nor the
        // bytes it retains may be request-controlled, and neither is paid while holding
        // the lock every other request waits on. A template that is not a packaged AWS SAM
        // template is refused here, so nothing is published for it.
        let parameter_definitions = crate::state::parse_template(input.template_body.as_ref())
            .map_err(|refusal| bad_request(refusal.message()))?;

        let mut store = state.lock();
        let Some(application) = store.applications.get_mut(&input.application_id) else {
            return Err(CreateApplicationVersionErrorModeled::NotFoundException(
                crate::state::not_found(
                    crate::state::ErrorCode::ApplicationNotFound,
                    crate::state::application_not_found_message(&input.application_id),
                ),
            )
            .into());
        };

        if application.versions.contains_key(&input.semantic_version) {
            // A duplicate version and a duplicate application are both 409
            // `ConflictException`, so the condition named in `errorCode` is the only
            // field that tells a client which of the two it hit.
            return Err(CreateApplicationVersionErrorModeled::ConflictException(
                crate::state::conflict(
                    crate::state::ErrorCode::VersionAlreadyExists,
                    format!(
                        "Cannot publish version {} for application {} because it already exists",
                        input.semantic_version, input.application_id
                    ),
                ),
            )
            .into());
        }
        // One application's version list grows once per call and is released only with
        // the application, so the growth is bounded rather than request-controlled.
        if application.versions.len() >= crate::state::MAX_VERSIONS_PER_APPLICATION {
            return Err(bad_request(format!(
                "This emulator retains at most {} versions per application.",
                crate::state::MAX_VERSIONS_PER_APPLICATION
            )));
        }

        let version = AppVersion {
            semantic_version: input.semantic_version.clone(),
            creation_time: crate::state::now_iso8601(),
            template_url: input.template_url.clone().unwrap_or_else(|| {
                crate::state::minted_artifact_url(
                    &host,
                    &region,
                    &application.name,
                    Some(&input.semantic_version),
                    Artifact::Template,
                )
            }),
            template_body: input.template_body.clone(),
            source_code_url: input.source_code_url.clone(),
            source_code_archive_url: input.source_code_archive_url.clone(),
            parameter_definitions,
            // The acknowledgement a deployment of this version will be held to, from the
            // template being published now.
            required_capabilities: crate::state::required_capabilities_of(
                input.template_body.as_ref(),
            ),
            resources_supported: true,
        };
        application.insert_version(version);

        // Answer from the STORED version.
        let Some(stored) = application.versions.get(&input.semantic_version) else {
            return Err(CreateApplicationVersionErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the created version was not persisted"),
            )
            .into());
        };
        let version = stored.version(&input.application_id);
        Ok(CreateApplicationVersionOutput {
            application_id: Some(version.application_id),
            creation_time: Some(version.creation_time),
            parameter_definitions: Some(version.parameter_definitions),
            required_capabilities: Some(version.required_capabilities),
            resources_supported: Some(version.resources_supported),
            semantic_version: Some(version.semantic_version),
            source_code_archive_url: version.source_code_archive_url,
            source_code_url: version.source_code_url,
            template_url: Some(version.template_url),
        })
    }
}

/// The 400 this operation answers for a parameter outside the contract.
fn bad_request(message: impl Into<String>) -> CreateApplicationVersionError {
    CreateApplicationVersionErrorModeled::BadRequestException(crate::state::bad_request(message))
        .into()
}
