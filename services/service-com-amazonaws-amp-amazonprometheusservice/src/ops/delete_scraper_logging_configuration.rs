// SPDX-License-Identifier: CC0-1.0

//! @op DeleteScraperLoggingConfiguration
//!
//! Deletes the logging configuration a scraper holds, out of the store this service
//! keeps it in
//! (`crate::state::ComAmazonawsAmpAmazonprometheusserviceState::scraper_logging`).
//!
//! The deletion is ACCEPTED (the modelled 202) rather than applied at once: the record
//! moves to `DELETING`, and the clock drops it at the instant this call records
//! ([`crate::state::ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`]), so
//! a caller reading straight after this call still sees the configuration being deleted.
//! That is the lifecycle `DeleteQueryLoggingConfiguration` gives a workspace's query
//! logging configuration, read from the same state constants.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteScraperLoggingConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-amp-amazonprometheusservice/smithy-model/com-amazonaws-amp-amazonprometheusservice.json")
//! ```
//!
//! Deletes the logging configuration for a Amazon Managed Service for Prometheus scraper.
//!
//! ## Input
//!
//! * `clientToken` (IdempotencyToken, len 1..64, pattern ^[!-~]+$, @idempotencyToken) — A
//!   unique, case-sensitive identifier that you provide to ensure the request is processed
//!   exactly once.
//! * `scraperId` (ScraperId, len 1..64, pattern ^[0-9A-Za-z][-.0-9A-Z_a-z]*$) **required** —
//!   The ID of the scraper whose logging configuration will be deleted.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `ConflictException` (HTTP 409) — The request would cause an inconsistent state.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resources that doesn't
//!   exist.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteScraperLoggingConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-amp-amazonprometheusservice.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteScraperLoggingConfigurationOutput;

impl DeleteScraperLoggingConfigurationOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/scrapers/{scraperId}/logging-configuration",
        Some(202u16),
    ));
}

pub static DELETE_SCRAPER_LOGGING_CONFIGURATION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteScraperLoggingConfigurationOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteScraperLoggingConfigurationOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteScraperLoggingConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.amp#DeleteScraperLoggingConfiguration")]
#[derive(Debug)]
pub enum DeleteScraperLoggingConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_scraper_logging_configuration::DeleteScraperLoggingConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteScraperLoggingConfigurationError = crate::scaffolded::Scaffolded<DeleteScraperLoggingConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteScraperLoggingConfigurationErrorModeled> for DeleteScraperLoggingConfigurationError {
    fn from(modeled: DeleteScraperLoggingConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteScraperLoggingConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteScraperLoggingConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteScraperLoggingConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteScraperLoggingConfiguration<Eng>
{
    type Input = DeleteScraperLoggingConfigurationInput;
    type Output = DeleteScraperLoggingConfigurationOutput;
    type Error = DeleteScraperLoggingConfigurationError;

    const NAME: &'static str = "DeleteScraperLoggingConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_SCRAPER_LOGGING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_SCRAPER_LOGGING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                DeleteScraperLoggingConfigurationErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the scraper store is unavailable".to_string(),
                        retry_after_seconds: None,
                    },
                )
                .into(),
            );
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.materialize_due(now);

        // The configuration is a child of the scraper, so a scraper this account does
        // not hold has no configuration to delete and is itself the missing resource.
        if !store.scrapers.contains_key(&input.scraper_id) {
            return Err(
                DeleteScraperLoggingConfigurationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::SCRAPER_NOT_FOUND_MESSAGE.to_string(),
                        resource_id: input.scraper_id.clone(),
                        resource_type: crate::state::SCRAPER_RESOURCE_TYPE.to_string(),
                    },
                )
                .into(),
            );
        }

        let Some(configuration) = store.scraper_logging.get_mut(&input.scraper_id) else {
            return Err(
                DeleteScraperLoggingConfigurationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::SCRAPER_LOGGING_NOT_FOUND_MESSAGE.to_string(),
                        resource_id: input.scraper_id.clone(),
                        resource_type: crate::state::SCRAPER_LOGGING_RESOURCE_TYPE.to_string(),
                    },
                )
                .into(),
            );
        };

        // The operation is `@idempotent` and `clientToken` carries `@idempotencyToken`:
        // re-sending the call that started the pending deletion is that same call, so it
        // is accepted again without restarting the span the removal is owed at. A
        // deletion already under way — the record reports `DELETING` and the clock owes
        // its removal — is likewise left alone.
        let deletion_under_way = configuration.status_code
            == crate::state::SCRAPER_LOGGING_STATUS_DELETING
            && configuration.transition_due.is_some();
        if !deletion_under_way {
            configuration.status_code = crate::state::SCRAPER_LOGGING_STATUS_DELETING.to_string();
            configuration.status_reason = None;
            configuration.modified_at = now;
            configuration.transition_due = Some(now + crate::state::TRANSITION_DELAY);
            configuration.client_token = input.client_token;
        }

        Ok(DeleteScraperLoggingConfigurationOutput)
    }
}
