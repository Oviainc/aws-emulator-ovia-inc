// SPDX-License-Identifier: CC0-1.0

//! @op CreateTieringConfiguration
//!
//! Stores the tiering configuration the request carries in [`crate::state`]
//! ([`crate::state::TieringConfigurationRecord`]) — the SAME record
//! `GetTieringConfiguration` reports, `ListTieringConfigurations` pages over and
//! `DeleteTieringConfiguration` removes.
//!
//! A tiering configuration is addressed by NAME, and that name "cannot be
//! changed after creation" (model doc for `TieringConfigurationName`), so the
//! name is what makes one configuration one resource: the store is keyed by it
//! and this create is the only writer of that key. However many creates of one
//! name arrive together, each resolves the store under the same lock, so exactly
//! ONE of them stores a configuration and every other is refused
//! `AlreadyExistsException` ([`crate::state::tiering_configuration_already_exists`])
//! — the condition this service words for every create that names a resource
//! the account already holds — and the survivor is what the reads report.
//!
//! A repeated `CreatorRequestId` is not such a loser: the member is the
//! idempotency token that "allows failed requests to be retried without the
//! risk of running the operation twice" (its model doc), so a retry carrying
//! the token of the create that stored the configuration answers with that
//! configuration.
//!
//! The selections the request carries are stored as submitted and reported back
//! by both reads; their modelled constraints — the `TieringDownSettingsInDays`
//! range among them — are refused before this handler runs.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateTieringConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Creates a tiering configuration. A tiering configuration enables automatic movement of
//! backup data to a lower-cost storage tier based on the age of backed-up objects in the
//! backup vault. Each vault can only have one vault-specific tiering configuration, in
//! addition to any global configuration that applies to all vaults.
//!
//! ## Input
//!
//! * `CreatorRequestId` (CreatorRequestId, @idempotencyToken) — This is a unique string that
//!   identifies the request and allows failed requests to be retried without the risk of
//!   running the operation twice. This parameter is optional. If used, this parameter must
//!   contain 1 to 50 alphanumeric or '-_.' characters.
//! * `TieringConfiguration` (TieringConfigurationInputForCreate) **required** — A tiering
//!   configuration must contain a unique TieringConfigurationName string you create and must
//!   contain a BackupVaultName and ResourceSelection. You may optionally include a
//!   CreatorRequestId string. The TieringConfigurationName is a unique string that is the
//!   name of the tiering configuration. This cannot be changed after creation, and it must
//!   consist of only alphanumeric characters and underscores.
//! * `TieringConfigurationTags` (Tags) — The tags to assign to the tiering configuration.
//!
//! ## Output
//!
//! * `CreationTime` (timestamp) — The date and time a tiering configuration was created, in
//!   Unix format and Coordinated Universal Time (UTC). The value of CreationTime is accurate
//!   to milliseconds. For example, the value 1516925490.087 represents Friday, January 26,
//!   2018 12:11:30.087AM.
//! * `TieringConfigurationArn` (ARN) — An Amazon Resource Name (ARN) that uniquely identifies
//!   the created tiering configuration.
//! * `TieringConfigurationName` (string) — This unique string is the name of the tiering
//!   configuration. The name cannot be changed after creation. The name consists of only
//!   alphanumeric characters and underscores. Maximum length is 200.
//!
//! ## Errors
//!
//! * `AlreadyExistsException` (HTTP 400) — The required resource already exists.
//! * `ConflictException` (HTTP 409) — Backup can't perform the action that you requested
//!   until it finishes performing a previous action. Try again later.
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `LimitExceededException` (HTTP 400) — A limit in the request has been exceeded; for
//!   example, a maximum number of items allowed in a request.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateTieringConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateTieringConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#CreateTieringConfiguration")]
#[derive(Debug)]
pub enum CreateTieringConfigurationErrorModeled {
    AlreadyExistsException(AlreadyExistsException),
    ConflictException(ConflictException),
    InvalidParameterValueException(InvalidParameterValueException),
    LimitExceededException(LimitExceededException),
    MissingParameterValueException(MissingParameterValueException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_tiering_configuration::CreateTieringConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateTieringConfigurationError = crate::scaffolded::Scaffolded<CreateTieringConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateTieringConfigurationErrorModeled> for CreateTieringConfigurationError {
    fn from(modeled: CreateTieringConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateTieringConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateTieringConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateTieringConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateTieringConfiguration<Eng>
{
    type Input = CreateTieringConfigurationInput;
    type Output = CreateTieringConfigurationOutput;
    type Error = CreateTieringConfigurationError;

    const NAME: &'static str = "CreateTieringConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_TIERING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_TIERING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateTieringConfigurationErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let configuration = input.tiering_configuration;
        let mut store = state.lock();

        // The replay first: a retry carrying the token of the create that
        // already stored a configuration takes no second action, so it is
        // resolved before the name check that such a retry would otherwise
        // trip.
        if let Some(existing) = input
            .creator_request_id
            .as_deref()
            .and_then(|token| store.tiering_configuration_by_creator_request_id(token))
        {
            return Ok(tiering_configuration_created(existing));
        }

        // The name is the resource's identity, and the store is keyed by it: a
        // create naming one the account already holds is refused, and the
        // refusal discloses nothing about the stored configuration. Decided
        // under the same lock the insert below takes, so concurrent creates of
        // one name have exactly one winner.
        if store
            .tiering_configurations
            .contains_key(&configuration.tiering_configuration_name)
        {
            return Err(CreateTieringConfigurationErrorModeled::AlreadyExistsException(
                crate::state::tiering_configuration_already_exists(),
            )
            .into());
        }

        // The store is in memory and every request can add to it, so the number
        // of configurations it keeps is bounded. AWS answers the same modelled
        // error once an account reaches its own quota.
        if store.tiering_configurations.len() >= crate::state::MAX_TIERING_CONFIGURATIONS {
            return Err(CreateTieringConfigurationErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "this emulator keeps at most {} tiering configurations",
                        crate::state::MAX_TIERING_CONFIGURATIONS
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let record = crate::state::TieringConfigurationRecord {
            arn: crate::state::tiering_configuration_arn(
                &region,
                &configuration.tiering_configuration_name,
            ),
            name: configuration.tiering_configuration_name,
            backup_vault_name: configuration.backup_vault_name,
            resource_selection: configuration.resource_selection,
            creator_request_id: input.creator_request_id,
            creation_time: now,
            // Nothing has updated this configuration yet, so there is no update
            // instant to report.
            last_updated_time: None,
            tags: input
                .tiering_configuration_tags
                .unwrap_or_default()
                .into_iter()
                .collect(),
        };
        let output = tiering_configuration_created(&record);
        store
            .tiering_configurations
            .insert(record.name.clone(), record);
        Ok(output)
    }
}

/// What this operation answers for the configuration it stored — the same
/// answer a replayed `CreatorRequestId` gets, so a retry cannot report
/// identifiers that differ from the ones the first call reported.
fn tiering_configuration_created(
    record: &crate::state::TieringConfigurationRecord,
) -> CreateTieringConfigurationOutput {
    CreateTieringConfigurationOutput {
        tiering_configuration_arn: Some(record.arn.clone()),
        tiering_configuration_name: Some(record.name.clone()),
        creation_time: Some(::aws_smithy_types::DateTime::from(record.creation_time)),
    }
}
