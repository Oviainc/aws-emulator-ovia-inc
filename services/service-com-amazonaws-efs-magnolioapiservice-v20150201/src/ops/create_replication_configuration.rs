// SPDX-License-Identifier: CC0-1.0

//! @op CreateReplicationConfiguration
//!
//! Creates a replication configuration for one source file system, either to a file
//! system that already exists or to one Amazon EFS creates for it. The record lives in
//! [`crate::state::ReplicationConfiguration`], keyed by its source file system, and
//! every description this operation and `DescribeReplicationConfigurations` answer with
//! is built from it.
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
//! # Modeled contract for `CreateReplicationConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Creates a replication conﬁguration to either a new or existing EFS file system. For more
//! information, see Amazon EFS replication in the Amazon EFS User Guide. The replication
//! configuration specifies the following: Source file system – The EFS file system that you
//! want to replicate. Destination file system – The destination file system to which the
//! source file system is replicated. There can only be one destination file system in a
//! replication configuration. A file system can be part of only one replication
//! configuration. The destination parameters for the replication configuration depend on
//! whether you are replicating to a new file system or to an existing file system, and if
//! you are replicating across Amazon Web Services accounts. See DestinationToCreate for
//! more information. This operation requires permissions for the
//! elasticfilesystem:CreateReplicationConfiguration action. Additionally, other permissions
//! are required depending on how you are replicating file systems. For more information,
//! see Required permissions for replication in the Amazon EFS User Guide.
//!
//! ## Input
//!
//! * `Destinations` (DestinationsToCreate) **required** — An array of destination
//!   configuration objects. Only one destination configuration object is supported.
//! * `SourceFileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — Specifies the Amazon EFS file system that you want to replicate. This
//!   file system cannot already be a source or destination file system in another replication
//!   configuration.
//!
//! ## Output
//!
//! * `CreationTime` (Timestamp) **required** — Describes when the replication configuration
//!   was created.
//! * `Destinations` (Destinations) **required** — An array of destination objects. Only one
//!   destination object is supported.
//! * `OriginalSourceFileSystemArn` (FileSystemArn) **required** — The Amazon Resource Name
//!   (ARN) of the original source EFS file system in the replication configuration.
//! * `SourceFileSystemArn` (FileSystemArn) **required** — The Amazon Resource Name (ARN) of
//!   the current source file system in the replication configuration.
//! * `SourceFileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   **required** — The ID of the source Amazon EFS file system that is being replicated.
//! * `SourceFileSystemOwnerId` (AwsAccountId, len 0..14, pattern
//!   ^(\d{12})|(\d{4}-\d{4}-\d{4})$) — ID of the Amazon Web Services account in which the
//!   source file system resides.
//! * `SourceFileSystemRegion` (RegionName, len 1..64, pattern
//!   ^[a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-{0,1}[0-9]{0,1}$) **required** — The
//!   Amazon Web Services Region in which the source EFS file system is located.
//!
//! ## Errors
//!
//! * `BadRequest` (HTTP 400) — Returned if the request is malformed or contains an error such
//!   as an invalid parameter value or a missing required parameter.
//! * `ConflictException` (HTTP 409) — Returned if the source file system in a replication is
//!   encrypted but the destination file system is unencrypted.
//! * `FileSystemLimitExceeded` (HTTP 403) — Returned if the Amazon Web Services account has
//!   already created the maximum number of file systems allowed per account.
//! * `FileSystemNotFound` (HTTP 404) — Returned if the specified FileSystemId value doesn't
//!   exist in the requester's Amazon Web Services account.
//! * `IncorrectFileSystemLifeCycleState` (HTTP 409) — Returned if the file system's lifecycle
//!   state is not "available".
//! * `InsufficientThroughputCapacity` (HTTP 503) — Returned if there's not enough capacity to
//!   provision additional throughput. This value might be returned when you try to create a
//!   file system in provisioned throughput mode, when you attempt to increase the provisioned
//!   throughput of an existing file system, or when you attempt to change an existing file
//!   system from Bursting Throughput to Provisioned Throughput mode. Try again later.
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `ReplicationNotFound` (HTTP 404) — Returned if the specified file system does not have a
//!   replication configuration.
//! * `ThroughputLimitExceeded` (HTTP 400) — Returned if the throughput mode or amount of
//!   provisioned throughput can't be changed because the throughput limit of 1024 MiB/s has
//!   been reached.
//! * `UnsupportedAvailabilityZone` (HTTP 400) — Returned if the requested Amazon EFS
//!   functionality is not available in the specified Availability Zone.
//! * `ValidationException` (HTTP 400) — Returned if the Backup service is not available in
//!   the Amazon Web Services Region in which the request was made.

#[allow(unused_imports)]
use super::types::*;

// This operation's not-found refusals carry the SENTENCE ALONE, with the error shape
// named in `x-amzn-errortype` and no `ErrorCode` member in the body. These explicit
// imports shadow the glob above, so the two variants below take that body instead of
// the generated shape; the rule lives on `crate::service::wire::MessageOnly`.
use crate::service::wire::{FileSystemNotFound, ReplicationNotFound};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateReplicationConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateReplicationConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#CreateReplicationConfiguration")]
#[derive(Debug)]
pub enum CreateReplicationConfigurationErrorModeled {
    BadRequest(BadRequest),
    ConflictException(ConflictException),
    FileSystemLimitExceeded(FileSystemLimitExceeded),
    FileSystemNotFound(FileSystemNotFound),
    IncorrectFileSystemLifeCycleState(IncorrectFileSystemLifeCycleState),
    InsufficientThroughputCapacity(InsufficientThroughputCapacity),
    InternalServerError(InternalServerError),
    ReplicationNotFound(ReplicationNotFound),
    ThroughputLimitExceeded(ThroughputLimitExceeded),
    UnsupportedAvailabilityZone(UnsupportedAvailabilityZone),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_replication_configuration::CreateReplicationConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateReplicationConfigurationError = crate::scaffolded::Scaffolded<CreateReplicationConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateReplicationConfigurationErrorModeled> for CreateReplicationConfigurationError {
    fn from(modeled: CreateReplicationConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateReplicationConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateReplicationConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateReplicationConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateReplicationConfiguration<Eng>
{
    type Input = CreateReplicationConfigurationInput;
    type Output = CreateReplicationConfigurationOutput;
    type Error = CreateReplicationConfigurationError;

    const NAME: &'static str = "CreateReplicationConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_REPLICATION_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_REPLICATION_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as efs;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_server_error());
        };
        let request_region = efs::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        // "An array of destination configuration objects. Only one destination
        // configuration object is supported." The modelled list carries no length
        // bound, so the documented count is enforced here.
        if input.destinations.len() != efs::MAX_REPLICATION_DESTINATIONS {
            return Err(bad_request(format!(
                "Destinations must hold exactly {} destination configuration object; got {}",
                efs::MAX_REPLICATION_DESTINATIONS,
                input.destinations.len()
            )));
        }
        let requested = input.destinations[0].clone();

        let mut state = state.lock();
        state.materialize_due_transitions(now);

        // "Specifies the Amazon EFS file system that you want to replicate" — the
        // member accepts either a file system id or its ARN.
        let source_id = efs::file_system_id_of(&input.source_file_system_id).to_string();
        let Some(source) = state.file_systems.get(&source_id).cloned() else {
            return Err(file_system_not_found(&source_id));
        };
        if source.life_cycle_state != efs::LIFE_CYCLE_AVAILABLE {
            return Err(incorrect_life_cycle_state(&source_id, &source.life_cycle_state));
        }
        // "This file system cannot already be a source or destination file system in
        // another replication configuration."
        if let Some(existing) = state.replication_of(&source_id) {
            return Err(conflict(format!(
                "File system {} is already part of the replication configuration of source \
                 file system {}",
                efs::quoted(&source_id),
                efs::quoted(&existing.source_file_system_id)
            )));
        }
        let source_region = efs::arn_region(&source.file_system_arn)
            .unwrap_or(request_region.as_str())
            .to_string();

        // The destination is either a file system that already exists or one Amazon EFS
        // creates for this configuration; the latter is committed below, past every
        // refusal, so a request that is turned away leaves no file system behind.
        let mut created_destination: Option<efs::FileSystem> = None;
        let destination = match requested.file_system_id.as_deref() {
            // "If you want to replicate to an existing file system ... provide the ID or
            // Amazon Resource Name (ARN) of the file system to which to replicate."
            Some(destination_ref) => {
                // These two members configure a file system Amazon EFS would create, so
                // naming an existing destination as well is contradictory.
                if requested.availability_zone_name.is_some() {
                    return Err(bad_request(
                        "AvailabilityZoneName configures a destination file system Amazon \
                         EFS creates, so it cannot be combined with Destinations[0].FileSystemId",
                    ));
                }
                if requested.kms_key_id.is_some() {
                    return Err(bad_request(
                        "KmsKeyId configures a destination file system Amazon EFS creates, so \
                         it cannot be combined with Destinations[0].FileSystemId",
                    ));
                }
                let destination_id = efs::file_system_id_of(destination_ref).to_string();
                // "There can only be one destination file system in a replication
                // configuration", and it is not the source itself.
                if destination_id == source_id {
                    return Err(bad_request(format!(
                        "Destinations[0].FileSystemId must name a file system other than the \
                         source file system {}",
                        efs::quoted(&source_id)
                    )));
                }
                let Some(existing) = state.file_systems.get(&destination_id).cloned() else {
                    return Err(file_system_not_found(&destination_id));
                };
                if existing.life_cycle_state != efs::LIFE_CYCLE_AVAILABLE {
                    return Err(incorrect_life_cycle_state(
                        &destination_id,
                        &existing.life_cycle_state,
                    ));
                }
                if let Some(other) = state.replication_of(&destination_id) {
                    return Err(conflict(format!(
                        "File system {} is already part of the replication configuration of \
                         source file system {}",
                        efs::quoted(&destination_id),
                        efs::quoted(&other.source_file_system_id)
                    )));
                }
                // "The file system's replication overwrite protection must be disabled."
                if existing.replication_overwrite_protection != efs::PROTECTION_DISABLED {
                    return Err(conflict(format!(
                        "File system {} has replication overwrite protection {}; it must be {} \
                         to be the destination of a replication configuration",
                        efs::quoted(&destination_id),
                        existing.replication_overwrite_protection,
                        efs::PROTECTION_DISABLED
                    )));
                }
                let destination_region = efs::arn_region(&existing.file_system_arn)
                    .unwrap_or(request_region.as_str())
                    .to_string();
                if let Some(region) = requested.region.as_deref() {
                    if region != destination_region {
                        return Err(bad_request(format!(
                            "Destinations[0].Region {region} does not hold file system {}, \
                             which is in {destination_region}",
                            efs::quoted(&destination_id)
                        )));
                    }
                }
                // `ConflictException` is "returned if the source file system in a
                // replication is encrypted but the destination file system is
                // unencrypted".
                if source.encrypted && !existing.encrypted {
                    return Err(conflict(format!(
                        "Source file system {} is encrypted, but destination file system {} is \
                         not",
                        efs::quoted(&source_id),
                        efs::quoted(&destination_id)
                    )));
                }
                efs::ReplicationDestination {
                    file_system_id: destination_id,
                    region: destination_region,
                    owner_id: existing.owner_id.clone(),
                    role_arn: requested.role_arn.clone(),
                    status: efs::REPLICATION_STATUS_ENABLING.to_string(),
                    enabled_at: now + efs::CREATING_FOR,
                }
            }
            // "If you want to replicate to a new file system, do not specify the File
            // System ID for the destination file system. Amazon EFS creates a new, empty
            // file system."
            None => {
                // "To create a file system that uses Regional storage, specify the
                // Amazon Web Services Region in which to create the destination file
                // system"; "For One Zone storage, specify the Availability Zone to
                // create the file system in". An Availability Zone name is its Region
                // code followed by a letter, so the zone fixes a Region too, and with
                // neither member the destination lands in the source's Region.
                let zone_region = requested
                    .availability_zone_name
                    .as_deref()
                    .and_then(efs::split_availability_zone_name)
                    .map(|(region, _)| region.to_string());
                let destination_region = requested
                    .region
                    .clone()
                    .or_else(|| zone_region.clone())
                    .unwrap_or_else(|| source_region.clone());
                if let Some(zone) = requested.availability_zone_name.as_deref() {
                    if zone_region.as_deref() != Some(destination_region.as_str()) {
                        return Err(unsupported_availability_zone(format!(
                            "AvailabilityZoneName {zone} is not an Availability Zone of \
                             Region {destination_region}"
                        )));
                    }
                }
                // The published quota: "File systems per account, per Region: 1,000".
                // It also bounds this store, so a repeated create cannot grow the
                // process without limit.
                if state.file_systems.len() >= efs::MAX_FILE_SYSTEMS_PER_REGION {
                    return Err(file_system_limit_exceeded(format!(
                        "this account already holds the maximum of {} file systems in \
                         Region {destination_region}",
                        efs::MAX_FILE_SYSTEMS_PER_REGION
                    )));
                }

                let ordinal = state.next_ordinal();
                let destination_id = format!("fs-{}", state.mint_hex());
                let file_system_arn = ::engine::emulator_arn(
                    "elasticfilesystem",
                    &destination_region,
                    &format!("file-system/{destination_id}"),
                );
                let owner_id = ::engine::emulator_account_id().to_string();
                let destination = efs::ReplicationDestination {
                    file_system_id: destination_id.clone(),
                    region: destination_region,
                    owner_id: owner_id.clone(),
                    role_arn: requested.role_arn.clone(),
                    status: efs::REPLICATION_STATUS_ENABLING.to_string(),
                    enabled_at: now + efs::CREATING_FOR,
                };
                created_destination = Some(efs::FileSystem {
                    ordinal,
                    file_system_id: destination_id.clone(),
                    file_system_arn,
                    owner_id,
                    // A file system Amazon EFS mints for a replication configuration
                    // carries no caller `CreationToken`; the one recorded here names
                    // this file system and no other, so the token store keeps its
                    // one-token-one-file-system invariant.
                    creation_token: destination_id,
                    creation_time: now,
                    available_at: now + efs::CREATING_FOR,
                    // A new file system "will have ... an initial lifecycle state
                    // creating"; the clock owes it `available`.
                    life_cycle_state: efs::LIFE_CYCLE_CREATING.to_string(),
                    name: None,
                    // The members no destination parameter carries take the documented
                    // CreateFileSystem defaults ("Default is generalPurpose", "Default
                    // is bursting").
                    performance_mode: efs::DEFAULT_PERFORMANCE_MODE.to_string(),
                    throughput_mode: efs::DEFAULT_THROUGHPUT_MODE.to_string(),
                    // No throughput change has been applied to a file system that was
                    // just created, so `UpdateFileSystem`'s 24-hour restriction does not
                    // bind yet.
                    last_restricted_throughput_change: None,
                    provisioned_throughput_in_mibps: None,
                    // A file system Amazon EFS mints here has no throughput change
                    // outstanding.
                    pending_throughput: None,
                    // "To use an Key Management Service key other than the default KMS
                    // key, then specify it", and "after the file system is created, you
                    // cannot change the KMS key": a destination file system is encrypted
                    // either way, under the named key or the EFS default one.
                    encrypted: true,
                    kms_key_id: Some(efs::kms_key_arn(
                        requested
                            .kms_key_id
                            .as_deref()
                            .unwrap_or(efs::DEFAULT_EFS_KMS_ALIAS),
                        &destination.region,
                    )),
                    availability_zone_name: requested.availability_zone_name.clone(),
                    availability_zone_id: requested
                        .availability_zone_name
                        .as_deref()
                        .map(efs::availability_zone_id),
                    // `CreateFileSystem$Backup`: "Default is false. However, if you
                    // specify an AvailabilityZoneName, the default is true."
                    backup: requested.availability_zone_name.is_some(),
                    size_in_bytes: 0,
                    // Nothing has metered this file system yet, which is what leaves
                    // `SizeInBytes.Timestamp` out of its description.
                    size_measured_at: None,
                    tags: Vec::new(),
                    mount_target_ids: Vec::new(),
                    // "REPLICATING – The file system is being used as the destination
                    // file system in a replication configuration. The file system is
                    // read-only and is modified only by EFS replication."
                    replication_overwrite_protection: efs::PROTECTION_REPLICATING.to_string(),
                    // A file system Amazon EFS mints here carries no
                    // LifecycleConfiguration until PutLifecycleConfiguration stores one.
                    lifecycle_policies: Vec::new(),
                });
                destination
            }
        };

        let replication = efs::ReplicationConfiguration {
            ordinal: state.next_ordinal(),
            source_file_system_id: source_id.clone(),
            source_file_system_arn: source.file_system_arn.clone(),
            // "The Amazon Resource Name (ARN) of the original source EFS file system in
            // the replication configuration": the file system the configuration was
            // created from, which is the current source until a configuration is failed
            // back onto another one.
            original_source_file_system_arn: source.file_system_arn.clone(),
            source_file_system_region: source_region,
            source_file_system_owner_id: source.owner_id.clone(),
            creation_time: now,
            destinations: vec![destination.clone()],
        };
        let description = replication.describe();

        // Past every refusal, so these are the first writes the request makes.
        match created_destination {
            Some(file_system) => {
                state
                    .creation_tokens
                    .insert(file_system.creation_token.clone(), file_system.file_system_id.clone());
                state
                    .file_systems
                    .insert(file_system.file_system_id.clone(), file_system);
            }
            // An existing destination becomes read-only while it is replicated into:
            // "REPLICATING – The file system is being used as the destination file system
            // in a replication configuration."
            None => {
                if let Some(existing) = state.file_systems.get_mut(&destination.file_system_id) {
                    existing.replication_overwrite_protection =
                        efs::PROTECTION_REPLICATING.to_string();
                }
            }
        }
        state.replications.insert(source_id, replication);
        Ok(description)
    }
}

/// A 400 `BadRequest`: "the request is malformed or contains an error such as an invalid
/// parameter value or a missing required parameter".
fn bad_request(message: impl Into<String>) -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::BadRequest(BadRequest {
        error_code: "BadRequest".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 409 `ConflictException`, the modelled conflict answer of this operation: a file
/// system that is already part of a replication configuration, a destination whose
/// replication overwrite protection is not disabled, or an encrypted source paired with
/// an unencrypted destination.
fn conflict(message: impl Into<String>) -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::ConflictException(ConflictException {
        error_code: Some("ConflictException".to_string()),
        message: Some(message.into()),
    })
    .into()
}

/// A 404 `FileSystemNotFound`: "the specified FileSystemId value doesn't exist in the
/// requester's Amazon Web Services account", with the one sentence every entry point
/// that resolves a `FileSystemId` renders.
fn file_system_not_found(file_system_id: &str) -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::FileSystemNotFound(FileSystemNotFound::new(
        crate::state::file_system_not_found_message(file_system_id),
    ))
    .into()
}

/// A 409 `IncorrectFileSystemLifeCycleState`: "returned if the file system's lifecycle
/// state is not available".
fn incorrect_life_cycle_state(
    file_system_id: &str,
    life_cycle_state: &str,
) -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::IncorrectFileSystemLifeCycleState(
        IncorrectFileSystemLifeCycleState {
            error_code: "IncorrectFileSystemLifeCycleState".to_string(),
            message: Some(format!(
                "File system {} is in life cycle state {life_cycle_state}, not {}",
                crate::state::quoted(file_system_id),
                crate::state::LIFE_CYCLE_AVAILABLE
            )),
        },
    )
    .into()
}

/// A 403 `FileSystemLimitExceeded`: "the Amazon Web Services account has already created
/// the maximum number of file systems allowed per account" — this operation creates one
/// when the destination names no existing file system.
fn file_system_limit_exceeded(message: impl Into<String>) -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::FileSystemLimitExceeded(FileSystemLimitExceeded {
        error_code: "FileSystemLimitExceeded".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 400 `UnsupportedAvailabilityZone`: "the requested Amazon EFS functionality is not
/// available in the specified Availability Zone".
fn unsupported_availability_zone(
    message: impl Into<String>,
) -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::UnsupportedAvailabilityZone(
        UnsupportedAvailabilityZone {
            error_code: "UnsupportedAvailabilityZone".to_string(),
            message: Some(message.into()),
        },
    )
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> CreateReplicationConfigurationError {
    CreateReplicationConfigurationErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
