// SPDX-License-Identifier: CC0-1.0

//! @op GetApplication
//! Reads one application back out of [`crate::state`] — the persisted record, and the
//! version the request names (or the application's latest when it names none).
//!
//! A pure read: it answers from the store and writes nothing, so two GetApplications
//! either side of any other call tell the same story.
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
//! # Modeled contract for `GetApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Gets the specified application.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `SemanticVersion` (__string) — The semantic version of the application to get.
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
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `NotFoundException` (HTTP 404) — The resource (for example, an access policy statement)
//!   specified in the request doesn't exist.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#GetApplication")]
#[derive(Debug)]
pub enum GetApplicationErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
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
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation reads anything, per the application the request
        // addresses; see [`crate::state::authorization`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.application_id,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetApplicationErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        let store = state.lock();
        let Some(application) = store.applications.get(&input.application_id) else {
            return Err(not_found(
                crate::state::ErrorCode::ApplicationNotFound,
                crate::state::application_not_found_message(&input.application_id),
            ));
        };

        // A named version that this application does not have is as absent as an
        // application that was never created — a different condition, so a different
        // `errorCode`, under the same modelled shape and status.
        let version = match input.semantic_version.as_deref() {
            Some(semantic_version) => match application.version_named(semantic_version) {
                Some(version) => Some(version),
                None => {
                    return Err(not_found(
                        crate::state::ErrorCode::VersionNotFound,
                        // The wording lives in `state` so every operation that resolves a
                        // version refuses an absent one identically.
                        crate::state::version_not_found_message(
                            &input.application_id,
                            semantic_version,
                        ),
                    ))
                }
            },
            None => application.latest(),
        };

        let summary = application.summary();
        Ok(GetApplicationOutput {
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
            version: version.map(|version| version.version(&input.application_id)),
        })
    }
}

/// The 404 this operation answers for an application, or a version of one, that the
/// store does not hold. `code` names WHICH of the two was absent: the body's `errorCode`
/// carries the condition, never the status the status line already carries.
fn not_found(code: crate::state::ErrorCode, message: String) -> GetApplicationError {
    GetApplicationErrorModeled::NotFoundException(crate::state::not_found(code, message)).into()
}
