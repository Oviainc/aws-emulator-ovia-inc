// SPDX-License-Identifier: CC0-1.0

//! @op CreateCloudFormationTemplate
//! Prepares the deployable CloudFormation template of ONE published version and stores it
//! in [`crate::state`] under the `TemplateId` it mints, so the `GetCloudFormationTemplate`
//! that follows reports exactly this record.
//!
//! The version prepared is the one the request names, or the application's latest when it
//! names none; an application the catalogue does not hold, and a version the application
//! does not carry, are both absent resources (HTTP 404) rather than invalid parameters,
//! while an application that carries NO version at all is a request the service cannot act
//! on (HTTP 400).
//!
//! The template is minted `PREPARING` — `Status` is the "Status of the template creation
//! workflow" — and the clock owes it `ACTIVE` at
//! [`crate::state::TEMPLATE_PREPARATION_SPAN`] and `EXPIRED` at
//! [`crate::state::TEMPLATE_LIFETIME`]; both are derived from the stored instants on every
//! read, so no read moves a status.
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
//! # Modeled contract for `CreateCloudFormationTemplate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Creates an AWS CloudFormation template.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//!
//! ## Output
//!
//! * `ApplicationId` (__string) — The application Amazon Resource Name (ARN).
//! * `CreationTime` (__string) — The date and time this resource was created.
//! * `ExpirationTime` (__string) — The date and time this template expires. Templates expire
//!   1 hour after creation.
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//! * `Status` (Status, one of: ACTIVE | EXPIRED | PREPARING) — Status of the template
//!   creation workflow. Possible values: PREPARING | ACTIVE | EXPIRED
//! * `TemplateId` (__string) — The UUID returned by CreateCloudFormationTemplate. Pattern:
//!   [0-9a-fA-F]{8}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{12}
//! * `TemplateUrl` (__string) — A link to the template that can be used to deploy the
//!   application using AWS CloudFormation.
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
// `CreateCloudFormationTemplateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateCloudFormationTemplateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#CreateCloudFormationTemplate")]
#[derive(Debug)]
pub enum CreateCloudFormationTemplateErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_cloud_formation_template::CreateCloudFormationTemplateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCloudFormationTemplateError = crate::scaffolded::Scaffolded<CreateCloudFormationTemplateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCloudFormationTemplateErrorModeled> for CreateCloudFormationTemplateError {
    fn from(modeled: CreateCloudFormationTemplateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCloudFormationTemplate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCloudFormationTemplate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCloudFormationTemplate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCloudFormationTemplate<Eng>
{
    type Input = CreateCloudFormationTemplateInput;
    type Output = CreateCloudFormationTemplateOutput;
    type Error = CreateCloudFormationTemplateError;

    const NAME: &'static str = "CreateCloudFormationTemplate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLOUD_FORMATION_TEMPLATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLOUD_FORMATION_TEMPLATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The template is reached only through its own application's path, so nothing in
        // this operation depends on the request's region.
        //
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation writes anything, per the application the request
        // addresses, so a refused call prepares nothing; see
        // [`crate::state::authorization`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.application_id,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateCloudFormationTemplateErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        // Drawn, not read from the store, so nothing below the lock has to compute it: a
        // uuid, as the member documents ("The UUID returned by
        // CreateCloudFormationTemplate. Pattern:
        // [0-9a-fA-F]{8}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{12}"),
        // minted by the crate's one id minter rather than a second generator.
        let template_id = crate::state::new_uuid();

        let mut store = state.lock();
        let Some(application) = store.applications.get(&input.application_id) else {
            return Err(not_found(
                crate::state::ErrorCode::ApplicationNotFound,
                crate::state::application_not_found_message(&input.application_id),
            ));
        };

        // A template is prepared from a PUBLISHED version: the one the request names, or
        // the latest when it names none. Either way a version the application does not
        // carry is an absent resource, under its own condition so a client can tell it
        // from an absent application.
        let prepared = match input.semantic_version.as_deref() {
            Some(semantic_version) => match application.version_named(semantic_version) {
                Some(version) => version,
                None => {
                    return Err(not_found(
                        crate::state::ErrorCode::VersionNotFound,
                        crate::state::version_not_found_message(
                            &input.application_id,
                            semantic_version,
                        ),
                    ))
                }
            },
            None => match application.latest() {
                Some(version) => version,
                None => {
                    // An application that carries NO version is not an absent resource:
                    // the caller named nothing that does not exist, the application simply
                    // has nothing publishable to prepare a template from. That is a
                    // parameter the service cannot act on — HTTP 400 — which is also what
                    // AWS answers when the same versionless application is asked for a
                    // change set, in that path's own words.
                    return Err(bad_request(NO_VERSION_TO_PREPARE_FROM))
                }
            },
        };
        let semantic_version = prepared.semantic_version.clone();
        // What this prepared template serves: the location of the template body published
        // with that version. The link is the version's own, so a template prepared from a
        // version answers with the body that version was published with, whether it was
        // published inline (a minted locator) or as a link the publisher supplied.
        let template_url = prepared.template_url.clone();

        if !store.make_room_for_template(now) {
            // The request is not the caller's mistake: it is this emulator that cannot
            // retain another prepared template, which is a server-side condition, so it is
            // answered 500 rather than as an invalid parameter.
            return Err(CreateCloudFormationTemplateErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error(format!(
                    "No more than {} prepared templates can be retained at once.",
                    crate::state::MAX_STORED_TEMPLATES
                )),
            )
            .into());
        }
        // Answer from the STORED template, so this response and the next
        // GetCloudFormationTemplate are two reads of one record. The id was minted for
        // THIS call, so its slot is vacant; a taken one would mean answering with somebody
        // else's template, which is a server-side failure rather than this call's answer.
        let stored = match store.templates.entry(template_id) {
            ::std::collections::btree_map::Entry::Vacant(slot) => {
                let prepared = crate::state::CloudFormationTemplate::prepared(
                    slot.key().clone(),
                    input.application_id.clone(),
                    semantic_version,
                    template_url,
                    now,
                );
                slot.insert(prepared)
            }
            ::std::collections::btree_map::Entry::Occupied(_) => {
                return Err(CreateCloudFormationTemplateErrorModeled::InternalServerErrorException(
                    crate::state::internal_server_error(
                        "a freshly minted TemplateId was already in use",
                    ),
                )
                .into())
            }
        };
        let report = stored.report(now);
        Ok(CreateCloudFormationTemplateOutput {
            application_id: Some(report.application_id),
            creation_time: Some(report.creation_time),
            expiration_time: Some(report.expiration_time),
            semantic_version: Some(report.semantic_version),
            status: Some(report.status),
            template_id: Some(report.template_id),
            template_url: Some(report.template_url),
        })
    }
}

/// The 404 this operation answers for an application, or a version of one, the store does
/// not hold. `code` names WHICH of the two was absent: the body's `errorCode` carries the
/// condition, never the status the status line already carries.
fn not_found(
    code: crate::state::ErrorCode,
    message: String,
) -> CreateCloudFormationTemplateError {
    CreateCloudFormationTemplateErrorModeled::NotFoundException(crate::state::not_found(
        code, message,
    ))
    .into()
}

/// The 400 this operation answers for an application that has published no version at
/// all, so there is nothing to prepare a template from.
///
/// The sentence is FIXED: it names neither the application nor a version, because there
/// is no version to name and this condition is about the operation that cannot be served,
/// not about a value in the request. It belongs to THIS deploy path alone — the sibling
/// `CreateCloudFormationChangeSet` states the same precondition in its own words — so it
/// is stated here rather than in a helper the two would share.
const NO_VERSION_TO_PREPARE_FROM: &str =
    "CloudFormation template can't be created from application with no version.";

/// The 400 this operation answers for a request it cannot act on: an application with no
/// version to prepare a template from.
fn bad_request(message: impl Into<String>) -> CreateCloudFormationTemplateError {
    CreateCloudFormationTemplateErrorModeled::BadRequestException(crate::state::bad_request(
        message,
    ))
    .into()
}
