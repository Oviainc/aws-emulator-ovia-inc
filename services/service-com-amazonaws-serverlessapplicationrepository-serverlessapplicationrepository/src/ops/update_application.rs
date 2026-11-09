// SPDX-License-Identifier: CC0-1.0

//! @op UpdateApplication
//! Rewrites the metadata of an application already in the catalogue — the members the
//! request carries, and only those — in [`crate::state`], and answers out of the stored
//! record so a later GetApplication/ListApplications reports the rewrite.
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
//! # Modeled contract for `UpdateApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Updates the specified application.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `Author` (__string) — The name of the author publishing the app. Minimum length=1.
//!   Maximum length=127. Pattern "^[a-z0-9](([a-z0-9]|-(?!-))*[a-z0-9])?$";
//!   (the model carries NO `@pattern` on this member and the service accepts any text
//!   within the documented length — see `crate::state::validate_author`)
//! * `Description` (__string) — The description of the application. Minimum length=1. Maximum
//!   length=256
//! * `HomePageUrl` (__string) — A URL with more information about the application, for
//!   example the location of your GitHub repository for the application.
//! * `Labels` (__listOf__string) — Labels to improve discovery of apps in search results.
//!   Minimum length=1. Maximum length=127. Maximum number of labels: 10 Pattern:
//!   "^[a-zA-Z0-9+\\-_:\\/@]+$";
//! * `ReadmeBody` (__string) — A text readme file in Markdown language that contains a more
//!   detailed description of the application and how it works. Maximum size 5 MB
//! * `ReadmeUrl` (__string) — A link to the readme file in Markdown language that contains a
//!   more detailed description of the application and how it works. Maximum size 5 MB
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
//! * `NotFoundException` (HTTP 404) — The resource (for example, an access policy statement)
//!   specified in the request doesn't exist.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;
use crate::state::Artifact;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#UpdateApplication")]
#[derive(Debug)]
pub enum UpdateApplicationErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_application::UpdateApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateApplicationError = crate::scaffolded::Scaffolded<UpdateApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateApplicationErrorModeled> for UpdateApplicationError {
    fn from(modeled: UpdateApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateApplication<Eng>
{
    type Input = UpdateApplicationInput;
    type Output = UpdateApplicationOutput;
    type Error = UpdateApplicationError;

    const NAME: &'static str = "UpdateApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation writes anything, per the application the request
        // addresses, so a refused patch changes nothing; see
        // [`crate::state::authorization`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.application_id,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdateApplicationErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        // The members this patch carries are the ones CreateApplication accepts, so they
        // are held to the same documented bounds, from the same place (`crate::state`) —
        // an absent member is not being rewritten and is not validated.
        if let Some(author) = input.author.as_deref() {
            crate::state::validate_author(author).map_err(bad_request)?;
        }
        if let Some(description) = input.description.as_deref() {
            crate::state::validate_description(description).map_err(bad_request)?;
        }
        if let Some(labels) = input.labels.as_deref() {
            crate::state::validate_labels(labels).map_err(bad_request)?;
        }
        // "You can specify only one of readmeBody and readmeUrl; otherwise, an error
        // results" (`smithy_query(query="doc", shape="CreateApplicationRequest",
        // member="ReadmeBody")`), and an inline body is bounded before it is retained.
        if input.readme_body.is_some() && input.readme_url.is_some() {
            let (body_member, url_member) = Artifact::Readme.members();
            return Err(bad_request(format!(
                "You can specify only one of {body_member} and {url_member}."
            )));
        }
        crate::state::bounded_bytes(
            "ReadmeBody",
            input.readme_body.as_ref(),
            crate::state::MAX_ARTIFACT_BYTES,
        )
        .map_err(bad_request)?;

        let mut store = state.lock();
        let Some(application) = store.applications.get_mut(&input.application_id) else {
            return Err(UpdateApplicationErrorModeled::NotFoundException(crate::state::not_found(
                crate::state::ErrorCode::ApplicationNotFound,
                crate::state::application_not_found_message(&input.application_id),
            ))
            .into());
        };

        // A PATCH rewrites the members it names and leaves every other one as stored. A
        // `Labels` list the request carries REPLACES the stored list, including when it is
        // empty: the member is the whole list, not an addition to it.
        if let Some(author) = input.author.clone() {
            application.author = author;
        }
        if let Some(description) = input.description.clone() {
            application.description = description;
        }
        if let Some(home_page_url) = input.home_page_url.clone() {
            application.home_page_url = Some(home_page_url);
        }
        if let Some(labels) = input.labels.clone() {
            application.labels = labels;
        }
        if input.readme_body.is_some() || input.readme_url.is_some() {
            // The readme is stored as the caller's link, or as the locator minted for a
            // body supplied inline — the same resolution the create performs, announcing
            // the endpoint THIS request arrived on so the locator is one the caller can
            // follow back here.
            let host = crate::state::request_host(ctx, &application.region);
            let readme_url = crate::state::resolve_artifact_url(
                input.readme_body.as_ref(),
                input.readme_url.as_ref(),
                &host,
                &application.region,
                &application.name,
                None,
                Artifact::Readme,
            );
            application.readme_url = readme_url;
            application.readme_body = input.readme_body.clone();
        }

        // Answer from the STORED record, so what this call returns is exactly what a
        // later GetApplication reads.
        let summary = application.summary();
        Ok(UpdateApplicationOutput {
            application_id: Some(summary.application_id),
            author: Some(summary.author),
            creation_time: summary.creation_time,
            description: Some(summary.description),
            home_page_url: summary.home_page_url,
            is_verified_author: Some(application.is_verified_author),
            labels: summary.labels,
            license_url: application.license_url.clone(),
            name: Some(summary.name),
            readme_url: application.readme_url.clone(),
            spdx_license_id: summary.spdx_license_id,
            verified_author_url: application.verified_author_url.clone(),
            version: application
                .latest()
                .map(|version| version.version(&input.application_id)),
        })
    }
}

/// The 400 this operation answers for a parameter outside the bounds the service
/// documents but the model does not express as a constraint trait.
fn bad_request(message: String) -> UpdateApplicationError {
    UpdateApplicationErrorModeled::BadRequestException(crate::state::bad_request(message)).into()
}
