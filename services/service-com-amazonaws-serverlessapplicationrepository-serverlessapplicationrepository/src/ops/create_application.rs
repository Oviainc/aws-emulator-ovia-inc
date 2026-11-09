// SPDX-License-Identifier: CC0-1.0

//! @op CreateApplication
//! Creates an application and, when the caller names a `SemanticVersion`, its first
//! version — persisted in [`crate::state`] and read back out of the store to build the
//! response, so a later GetApplication/ListApplications answers with what was created.
//!
//! This is the ONLY file to touch for this operation.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Creates an application, optionally including an AWS SAM file to create the first
//! application version in the same call.
//!
//! ## Input
//!
//! * `Author` (__string) **required** — The name of the author publishing the app. Minimum
//!   length=1. Maximum length=127. Pattern "^[a-z0-9](([a-z0-9]|-(?!-))*[a-z0-9])?$";
//!   (the model carries NO `@pattern` on this member and the service accepts any text
//!   within the documented length — see `crate::state::validate_author`)
//! * `Description` (__string) **required** — The description of the application. Minimum
//!   length=1. Maximum length=256
//! * `HomePageUrl` (__string) — A URL with more information about the application, for
//!   example the location of your GitHub repository for the application.
//! * `Labels` (__listOf__string) — Labels to improve discovery of apps in search results.
//!   Minimum length=1. Maximum length=127. Maximum number of labels: 10 Pattern:
//!   "^[a-zA-Z0-9+\\-_:\\/@]+$";
//! * `LicenseBody` (__string) — A local text file that contains the license of the app that
//!   matches the spdxLicenseID value of your application. The file has the format
//!   file://<path>/<filename>. Maximum size 5 MB You can specify only one of licenseBody and
//!   licenseUrl; otherwise, an error results.
//! * `LicenseUrl` (__string) — A link to the S3 object that contains the license of the app
//!   that matches the spdxLicenseID value of your application. Maximum size 5 MB You can
//!   specify only one of licenseBody and licenseUrl; otherwise, an error results.
//! * `Name` (__string) **required** — The name of the application that you want to publish.
//!   Minimum length=1. Maximum length=140 Pattern: "[a-zA-Z0-9\\-]+";
//! * `ReadmeBody` (__string) — A local text readme file in Markdown language that contains a
//!   more detailed description of the application and how it works. The file has the format
//!   file://<path>/<filename>. Maximum size 5 MB You can specify only one of readmeBody and
//!   readmeUrl; otherwise, an error results.
//! * `ReadmeUrl` (__string) — A link to the S3 object in Markdown language that contains a
//!   more detailed description of the application and how it works. Maximum size 5 MB You can
//!   specify only one of readmeBody and readmeUrl; otherwise, an error results.
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//! * `SourceCodeArchiveUrl` (__string) — A link to the S3 object that contains the ZIP
//!   archive of the source code for this version of your application. Maximum size 50 MB
//! * `SourceCodeUrl` (__string) — A link to a public repository for the source code of your
//!   application, for example the URL of a specific GitHub commit.
//! * `SpdxLicenseId` (__string) — A valid identifier from https://spdx.org/licenses/.
//! * `TemplateBody` (__string) — The local raw packaged AWS SAM template file of your
//!   application. The file has the format file://<path>/<filename>. You can specify only one
//!   of templateBody and templateUrl; otherwise an error results.
//! * `TemplateUrl` (__string) — A link to the S3 object containing the packaged AWS SAM
//!   template of your application. You can specify only one of templateBody and templateUrl;
//!   otherwise an error results.
//!
//! ## Output
//!
//! * `ApplicationId` (__string) — The application Amazon Resource Name (ARN).
//! * `Author` (__string) — The name of the author publishing the app. Minimum length=1.
//!   Maximum length=127. Pattern "^[a-z0-9](([a-z0-9]|-(?!-))*[a-z0-9])?$";
//! * `CreationTime` (__string) — The date and time this resource was created.
//! * `Description` (__string) — The description of the application. Minimum length=1. Maximum
//!   length=256
//! * `HomePageUrl` (__string) — A URL with more information about the application, for
//!   example the location of your GitHub repository for the application.
//! * `IsVerifiedAuthor` (__boolean) — Whether the author of this application has been
//!   verified. This means means that AWS has made a good faith review, as a reasonable and
//!   prudent service provider, of the information provided by the requester and has confirmed
//!   that the requester's identity is as claimed.
//! * `Labels` (__listOf__string) — Labels to improve discovery of apps in search results.
//!   Minimum length=1. Maximum length=127. Maximum number of labels: 10 Pattern:
//!   "^[a-zA-Z0-9+\\-_:\\/@]+$";
//! * `LicenseUrl` (__string) — A link to a license file of the app that matches the
//!   spdxLicenseID value of your application. Maximum size 5 MB
//! * `Name` (__string) — The name of the application. Minimum length=1. Maximum length=140
//!   Pattern: "[a-zA-Z0-9\\-]+";
//! * `ReadmeUrl` (__string) — A link to the readme file in Markdown language that contains a
//!   more detailed description of the application and how it works. Maximum size 5 MB
//! * `SpdxLicenseId` (__string) — A valid identifier from https://spdx.org/licenses/.
//! * `VerifiedAuthorUrl` (__string) — The URL to the public profile of a verified author.
//!   This URL is submitted by the author.
//! * `Version` (Version) — Version information about the application.
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
use crate::state::{AppVersion, Application, Artifact};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#CreateApplication")]
#[derive(Debug)]
pub enum CreateApplicationErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_application::CreateApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateApplicationError = crate::scaffolded::Scaffolded<CreateApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateApplicationErrorModeled> for CreateApplicationError {
    fn from(modeled: CreateApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateApplication<Eng>
{
    type Input = CreateApplicationInput;
    type Output = CreateApplicationOutput;
    type Error = CreateApplicationError;

    const NAME: &'static str = "CreateApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation writes anything, so a refused create leaves no
        // application behind. This call addresses no existing application —
        // `serverlessrepo:CreateApplication` is an action AWS defines over no resource type
        // — so the decision is made about
        // [`crate::state::authorization::ANY_RESOURCE`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            crate::state::authorization::ANY_RESOURCE,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_error("the service state is unavailable"));
        };

        // The model expresses none of these as constraint traits, so the boundary
        // validator passes them and the service enforces the documented bounds here, as
        // the one error it models for an invalid parameter. The bounds themselves live in
        // `crate::state`, shared with `UpdateApplication`, which rewrites the same
        // members.
        crate::state::validate_name(&input.name).map_err(bad_request)?;
        crate::state::validate_author(&input.author).map_err(bad_request)?;
        crate::state::validate_description(&input.description).map_err(bad_request)?;
        crate::state::validate_labels(input.labels.as_deref().unwrap_or_default())
            .map_err(bad_request)?;
        if let Some(semantic_version) = input.semantic_version.as_deref() {
            if !crate::state::is_semantic_version(semantic_version) {
                return Err(bad_request(crate::state::INVALID_SEMANTIC_VERSION_MESSAGE));
            }
        }
        exclusive(
            input.license_body.as_ref(),
            input.license_url.as_ref(),
            Artifact::License,
        )?;
        exclusive(
            input.readme_body.as_ref(),
            input.readme_url.as_ref(),
            Artifact::Readme,
        )?;
        exclusive(
            input.template_body.as_ref(),
            input.template_url.as_ref(),
            Artifact::Template,
        )?;
        // An inline body is retained for the life of the record, so its size is bounded
        // before it is stored, at the maximum the member documents.
        crate::state::bounded_bytes(
            "LicenseBody",
            input.license_body.as_ref(),
            crate::state::MAX_ARTIFACT_BYTES,
        )
        .map_err(bad_request)?;
        crate::state::bounded_bytes(
            "ReadmeBody",
            input.readme_body.as_ref(),
            crate::state::MAX_ARTIFACT_BYTES,
        )
        .map_err(bad_request)?;
        // The template is the third inline body this call retains, and its ceiling is the
        // template one (a packaged CloudFormation template's documented maximum), not the
        // 5 MB of a license or readme. Checked here, before the body is parsed, so neither
        // the parse nor the retained bytes is request-controlled.
        crate::state::bounded_bytes(
            "TemplateBody",
            input.template_body.as_ref(),
            crate::state::MAX_TEMPLATE_BYTES,
        )
        .map_err(bad_request)?;

        // Read BEFORE the store is locked: the template is caller-supplied, so the work
        // it costs must not be paid while holding the lock every other request waits on.
        // A `SemanticVersion` makes this call publish a version, so the template it
        // publishes is held to the same contract CreateApplicationVersion holds one to.
        let parameter_definitions = crate::state::parse_template(input.template_body.as_ref())
            .map_err(|refusal| bad_request(refusal.message()))?;

        let region = crate::state::request_region(ctx);
        // The endpoint this request arrived on: the host a locator minted for an inline
        // body announces, because that locator has to be one the caller can follow back
        // here (`crate::state::minted_artifact_url`).
        let host = crate::state::request_host(ctx, &region);
        let application_id = crate::state::application_arn(&region, &input.name);
        let name_key = crate::state::name_key(&region, &input.name);

        let mut store = state.lock();
        // A name already in the catalogue is the modelled conflict (HTTP 409), and the
        // message names the application that is in the way, which is the ARN minted from
        // the requested name.
        if store.name_index.contains_key(&name_key) {
            // The condition is "this name is already published", so that is what the
            // body's `errorCode` names — not the 409 the status line already carries.
            return Err(CreateApplicationErrorModeled::ConflictException(crate::state::conflict(
                crate::state::ErrorCode::ApplicationAlreadyExists,
                format!("Application with id {application_id} already exists."),
            ))
            .into());
        }
        // The store grows once per create and is released only by a caller's delete, so
        // the growth is bounded rather than request-controlled.
        if store.applications.len() >= crate::state::MAX_STORED_APPLICATIONS {
            return Err(bad_request(format!(
                "This emulator retains at most {} applications. Delete one before creating \
                 another.",
                crate::state::MAX_STORED_APPLICATIONS
            )));
        }

        let creation_time = crate::state::now_iso8601();
        let mut application = Application {
            application_id: application_id.clone(),
            name: input.name.clone(),
            author: input.author.clone(),
            description: input.description.clone(),
            home_page_url: input.home_page_url.clone(),
            labels: input.labels.clone().unwrap_or_default(),
            license_url: crate::state::resolve_artifact_url(
                input.license_body.as_ref(),
                input.license_url.as_ref(),
                &host,
                &region,
                &input.name,
                None,
                Artifact::License,
            ),
            license_body: input.license_body.clone(),
            readme_url: crate::state::resolve_artifact_url(
                input.readme_body.as_ref(),
                input.readme_url.as_ref(),
                &host,
                &region,
                &input.name,
                None,
                Artifact::Readme,
            ),
            readme_body: input.readme_body.clone(),
            spdx_license_id: input.spdx_license_id.clone(),
            // Author verification is a manual AWS review, not something a publish call
            // performs, so a freshly published application is never a verified author's
            // and has no verified-author profile to report.
            is_verified_author: false,
            verified_author_url: None,
            creation_time: creation_time.clone(),
            region: region.clone(),
            versions: ::std::collections::BTreeMap::new(),
            latest_version: None,
            // A published application starts private: a permission policy is granted
            // only by a later `PutApplicationPolicy`.
            policy_statements: Vec::new(),
        };

        // "optionally including an AWS SAM file to create the first application version
        // in the same call": a `SemanticVersion` makes this call create that version too.
        if let Some(semantic_version) = input.semantic_version.clone() {
            let version = AppVersion {
                semantic_version: semantic_version.clone(),
                creation_time,
                template_url: crate::state::resolve_artifact_url(
                    input.template_body.as_ref(),
                    input.template_url.as_ref(),
                    &host,
                    &region,
                    &input.name,
                    Some(&semantic_version),
                    Artifact::Template,
                )
                .unwrap_or_else(|| {
                    crate::state::minted_artifact_url(
                        &host,
                        &region,
                        &input.name,
                        Some(&semantic_version),
                        Artifact::Template,
                    )
                }),
                template_body: input.template_body.clone(),
                source_code_url: input.source_code_url.clone(),
                source_code_archive_url: input.source_code_archive_url.clone(),
                parameter_definitions,
                // The acknowledgement a deployment of this version will be held to, from
                // the template being published now.
                required_capabilities: crate::state::required_capabilities_of(
                    input.template_body.as_ref(),
                ),
                resources_supported: true,
            };
            application.insert_version(version);
        }

        store.name_index.insert(name_key, application_id.clone());
        store.applications.insert(application_id.clone(), application);

        // Answer from the STORED record, so what this call returns is exactly what a
        // later GetApplication reads.
        let Some(stored) = store.applications.get(&application_id) else {
            return Err(internal_error("the created application was not persisted"));
        };
        let summary = stored.summary();
        Ok(CreateApplicationOutput {
            application_id: Some(summary.application_id),
            author: Some(summary.author),
            creation_time: summary.creation_time,
            description: Some(summary.description),
            home_page_url: summary.home_page_url,
            is_verified_author: Some(stored.is_verified_author),
            labels: summary.labels,
            license_url: stored.license_url.clone(),
            name: Some(summary.name),
            readme_url: stored.readme_url.clone(),
            spdx_license_id: summary.spdx_license_id,
            verified_author_url: stored.verified_author_url.clone(),
            version: stored
                .latest()
                .map(|version| version.version(&application_id)),
        })
    }
}

/// The 500 this operation answers when the shared store cannot be reached. Never a
/// panic: the framework renders a panic as a retryable 500 the real service never gives.
fn internal_error(message: &str) -> CreateApplicationError {
    CreateApplicationErrorModeled::InternalServerErrorException(
        crate::state::internal_server_error(message),
    )
    .into()
}

/// The 400 this operation answers for a parameter the model does not constrain but the
/// service documents.
fn bad_request(message: impl Into<String>) -> CreateApplicationError {
    CreateApplicationErrorModeled::BadRequestException(crate::state::bad_request(message)).into()
}

/// "You can specify only one of {body} and {url}; otherwise an error results"
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="TemplateBody")`,
/// and the same sentence on the license and readme pairs).
fn exclusive(
    body: Option<&String>,
    url: Option<&String>,
    artifact: Artifact,
) -> Result<(), CreateApplicationError> {
    if body.is_some() && url.is_some() {
        let (body_member, url_member) = artifact.members();
        return Err(bad_request(format!(
            "You can specify only one of {body_member} and {url_member}."
        )));
    }
    Ok(())
}
