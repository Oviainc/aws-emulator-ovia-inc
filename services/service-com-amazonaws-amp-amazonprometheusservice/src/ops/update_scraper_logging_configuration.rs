// SPDX-License-Identifier: CC0-1.0

//! @op UpdateScraperLoggingConfiguration
//!
//! Replaces the logging configuration a scraper HOLDS, in the store this service keeps it
//! in (`crate::state::RegionalState::scraper_logging`), and reports the status of the
//! record it wrote.
//!
//! The update is ACCEPTED (the modelled 202) rather than applied at once: the record
//! moves to `UPDATING`, and the clock settles it back to `ACTIVE` at the instant this
//! call records
//! ([`crate::state::ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`]), so a
//! caller reading straight after this call still sees the configuration being updated.
//! That is the lifecycle `UpdateQueryLoggingConfiguration` gives a workspace's query
//! logging configuration, read from the same state constants.
//!
//! A scraper this account does not hold, and a scraper that holds no logging
//! configuration, are each the modelled `ResourceNotFoundException` naming the resource
//! that is missing — the same two refusals `DescribeScraperLoggingConfiguration` and
//! `DeleteScraperLoggingConfiguration` answer, since the three address one record.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateScraperLoggingConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-amp-amazonprometheusservice/smithy-model/com-amazonaws-amp-amazonprometheusservice.json")
//! ```
//!
//! Updates the logging configuration for a Amazon Managed Service for Prometheus scraper.
//!
//! ## Input
//!
//! * `loggingDestination` (ScraperLoggingDestination) **required** — The destination where
//!   scraper logs will be sent.
//! * `scraperComponents` (ScraperComponents, len 1..) — The list of scraper components to
//!   configure for logging.
//! * `scraperId` (ScraperId, len 1..64, pattern ^[0-9A-Za-z][-.0-9A-Z_a-z]*$) **required** —
//!   The ID of the scraper whose logging configuration will be updated.
//!
//! ## Output
//!
//! * `status` (ScraperLoggingConfigurationStatus) **required** — The status of the updated
//!   scraper logging configuration.
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
// `UpdateScraperLoggingConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-amp-amazonprometheusservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateScraperLoggingConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.amp#UpdateScraperLoggingConfiguration")]
#[derive(Debug)]
pub enum UpdateScraperLoggingConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_scraper_logging_configuration::UpdateScraperLoggingConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateScraperLoggingConfigurationError = crate::scaffolded::Scaffolded<UpdateScraperLoggingConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateScraperLoggingConfigurationErrorModeled> for UpdateScraperLoggingConfigurationError {
    fn from(modeled: UpdateScraperLoggingConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateScraperLoggingConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateScraperLoggingConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateScraperLoggingConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateScraperLoggingConfiguration<Eng>
{
    type Input = UpdateScraperLoggingConfigurationInput;
    type Output = UpdateScraperLoggingConfigurationOutput;
    type Error = UpdateScraperLoggingConfigurationError;

    const NAME: &'static str = "UpdateScraperLoggingConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_SCRAPER_LOGGING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_SCRAPER_LOGGING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                UpdateScraperLoggingConfigurationErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the scraper store is unavailable".to_string(),
                        retry_after_seconds: None,
                    },
                )
                .into(),
            );
        };

        let now = ::std::time::SystemTime::now();
        // This service is REGIONAL, so the store this call acts on is the one of the
        // Region the request was signed for: a scraper created in another Region is not
        // reachable from this endpoint and is not found here.
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();
        // Every lifecycle transition the clock already owed at this instant was owed
        // whether or not anybody called, so this call acts on the record as of now rather
        // than as of the last write.
        store.materialize_due(now);

        // The configuration is a child of the scraper, so a scraper this account does not
        // hold has no configuration to update and is itself the missing resource — and a
        // Region this account holds nothing in holds no scraper either.
        let Some(store) = store
            .held_region_mut(&region)
            .filter(|store| store.scrapers.contains_key(&input.scraper_id))
        else {
            return Err(
                UpdateScraperLoggingConfigurationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::SCRAPER_NOT_FOUND_MESSAGE.to_string(),
                        resource_id: input.scraper_id.clone(),
                        resource_type: crate::state::SCRAPER_RESOURCE_TYPE.to_string(),
                    },
                )
                .into(),
            );
        };

        // This operation UPDATES a configuration, so there has to be one to update: a
        // scraper that holds none is the modelled `ResourceNotFoundException` naming the
        // configuration, exactly as `UpdateQueryLoggingConfiguration` refuses a workspace
        // that holds no query logging configuration.
        let Some(configuration) = store.scraper_logging.get_mut(&input.scraper_id) else {
            return Err(
                UpdateScraperLoggingConfigurationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::SCRAPER_LOGGING_NOT_FOUND_MESSAGE.to_string(),
                        resource_id: input.scraper_id.clone(),
                        resource_type: crate::state::SCRAPER_LOGGING_RESOURCE_TYPE.to_string(),
                    },
                )
                .into(),
            );
        };

        // A configuration whose deletion has already started is on its way out, so
        // updating it would leave an inconsistent state — `ConflictException` (409),
        // which this operation models.
        if configuration.status_code == crate::state::SCRAPER_LOGGING_STATUS_DELETING {
            return Err(
                UpdateScraperLoggingConfigurationErrorModeled::ConflictException(
                    ConflictException {
                        message: format!(
                            "The scraper logging configuration is {}",
                            crate::state::SCRAPER_LOGGING_STATUS_DELETING,
                        ),
                        resource_id: input.scraper_id.clone(),
                        resource_type: crate::state::SCRAPER_LOGGING_RESOURCE_TYPE.to_string(),
                    },
                )
                .into(),
            );
        }

        // `loggingDestination` is `@required`, so every accepted update carries the whole
        // destination and REPLACES the stored one. `scraperComponents` is optional, and
        // modelled `len 1..` and `@required` on the record a describe reports: a
        // configuration always covers at least one component, so a request carrying no
        // list leaves the stored one standing rather than emptying it.
        configuration.logging_destination = input.logging_destination;
        if let Some(scraper_components) = input.scraper_components {
            configuration.scraper_components = scraper_components;
        }
        // The update is asynchronous (the modelled 202): the record moves to `UPDATING`
        // here and the clock owes its settling, which
        // `ComAmazonawsAmpAmazonprometheusserviceState::materialize_due` applies.
        configuration.status_code = crate::state::SCRAPER_LOGGING_STATUS_UPDATING.to_string();
        // Nothing has failed about the configuration this call wrote, so it carries no
        // reason for a failure.
        configuration.status_reason = None;
        // This call CHANGED the stored record, which is what
        // `DescribeScraperLoggingConfigurationResponse$modifiedAt` reports.
        configuration.modified_at = now;
        configuration.transition_due = Some(now + crate::state::TRANSITION_DELAY);

        // Reported off the record this call wrote, through the one derivation every reader
        // and writer of this configuration answers with.
        let response = UpdateScraperLoggingConfigurationOutput {
            status: crate::state::scraper_logging_status(configuration),
        };
        // This call CHANGED this service's state, so it leaves the audit record every
        // mutating handler of this crate leaves: the operation, the resource it changed
        // and the access-key id the REQUEST presented — see
        // [`crate::state::record_state_change`]. The configuration has no ARN of its own,
        // so the resource is the scraper it belongs to.
        let scraper_arn = crate::state::scraper_arn(&region, &input.scraper_id);
        crate::state::record_state_change(Self::NAME, &scraper_arn, ctx.parts());
        Ok(response)
    }
}
