// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Tenant-agnostic telemetry for generated emulators.
//!
//! The crate aggregates operation counts, outcomes, folded error types,
//! latencies, and coverage gaps in memory, then sends closed-schema NDJSON to
//! endpoints injected by the project scaffold. A dedicated worker owns all
//! network and SQLite I/O, so recording never blocks an emulator request.
//!
//! Identity is also injected: [`TelemetryOptions`] carries the canonical
//! [`ProjectId`] the emulator was generated for and its [`ProductVersion`].

mod client;
mod config;
mod metadata;
mod outbox;
pub mod policy;
#[cfg(test)]
mod test_support;
mod types;
mod wire;

#[cfg(test)]
mod backend_contract;
#[cfg(test)]
mod integration_tests;

pub use client::{
    flush_at_process_exit, global, init_global, record_generated_operation_with_handle,
    recording_active, ModelOperation, OperationFailure, OperationHandle, TelemetryClient,
    TelemetryOptions, TelemetryRequestGuard,
};
pub use config::{
    resolve_config_file, Endpoint, EndpointError, ServerConfig, Switch, SwitchSource,
};
pub use metadata::{sanitize_request_metadata, SanitizedRequestMetadata};
pub use policy::{
    ConsentMode, CLIENT_VERSION, CONFIG_DIR_NAME, EMULATOR_CONFIG_ENV, EMULATOR_CONFIG_FILE_NAME,
    NOTICE_LOG_TARGET, SCHEMA_VERSION, TELEMETRY_CONSENT_MODE, TELEMETRY_DEFAULT_ENABLED,
    TELEMETRY_ENDPOINT_ENV, TELEMETRY_STATE_DIR_NAME, TELEMETRY_SWITCH_ENV,
};
pub use types::{
    AgentClient, Arch, BatchId, ConfigCacheError, ConfigEtag, ContainerRuntime, CrashType,
    DimensionError, DimensionErrorReason, ErrorType, InstallChannel, InstallationId, InvalidUuidId,
    LanguageVersion, MemoryBucket, MetricType, MinClientVersion, Namespace, OperationName,
    OsFamily, Outcome, ProductVersion, ProductVersionError, ProjectId, ProjectIdError, Region,
    RegionError, RunMode, SdkDimension, SdkName, SdkVersion, SessionId, StartupOutcome,
    StateArtifact, StatePurgeError, TelemetryFeatureFlags, VersionStringError,
};
