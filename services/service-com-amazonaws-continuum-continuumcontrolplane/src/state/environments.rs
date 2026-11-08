// SPDX-License-Identifier: CC0-1.0

//! The stored environment (`/environments`), and the projections the five
//! environment operations answer with.
//!
//! One record per `environmentId` holds everything a read of the environment
//! reports: its minted id and ARN, its name, its status and the instants that
//! status, the record and its creation were last written, plus the configuration
//! `CreateEnvironment` persisted and `UpdateEnvironment` merges into. Every
//! response member of `CreateEnvironment`, `GetEnvironment`,
//! `UpdateEnvironment`, `ListEnvironments` comes out of this record, so the
//! five cannot disagree about one environment.

use std::ops::Bound;
use std::time::SystemTime;

use crate::ops::types::{
    CodeConfigurationDetails, CreateEnvironmentConfiguration, CreateEnvironmentOutput,
    EnvironmentConfiguration, EnvironmentConfigurationDetails, EnvironmentListEntry,
    EnvironmentSourceConfiguration, GetEnvironmentOutput, ResourceNotFoundException,
    UpdateEnvironmentConfiguration, UpdateEnvironmentOutput,
};
use crate::state::{
    encode_page_token, timestamp, ComAmazonawsContinuumContinuumcontrolplaneState, ARN_SERVICE,
};

/// The only status `EnvironmentStatus` admits (`ENVIRONMENT_STATUS_VALUES`, from
/// `smithy_query shape=EnvironmentStatus`), which `CreateEnvironment`'s output
/// documents as "Always ACTIVE on successful creation" — so an environment is
/// ready when it is created and the clock owes it no later transition.
pub const ENVIRONMENT_STATUS_ACTIVE: &str = "ACTIVE";

/// The scan cadence an environment created without one runs on:
/// `EnvironmentConfiguration$scanFrequency` — "Defaults to P1D when creating an
/// environment."
pub const DEFAULT_SCAN_FREQUENCY: &str = "P1D";

/// Whether a new environment creates patches for proposed remediations:
/// `EnvironmentConfiguration$patchCreationPolicy` — "Defaults to DISABLED."
pub const DEFAULT_PATCH_CREATION_POLICY: &str = "DISABLED";

/// How many environments one account can hold here, after which
/// `CreateEnvironment` answers its modelled `ServiceQuotaExceededException`
/// rather than growing this process's memory for as long as a caller keeps
/// asking.
///
/// An emulator-chosen bound, not an AWS-published limit: the model declares the
/// quota error but no quota value, so this number caps the store rather than
/// reproducing a documented service limit.
pub const MAX_ENVIRONMENTS: usize = 100;

/// `arn:aws:continuum:{region}:{account}:environments/{environmentId}` —
/// `EnvironmentArn`'s pattern
/// (`^arn:[a-z0-9-]+:continuum:[a-z0-9-]+:[0-9]{12}:environments/[a-z0-9]+$`).
pub fn environment_arn(region: &str, environment_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("environments/{environment_id}"),
    )
}

/// How every environment operation words an id that names no stored
/// environment: the model's own `ResourceNotFoundException` text, with the
/// identifier the request carried.
pub fn environment_not_found(environment_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "Environment {environment_id} was not found. Verify that the resource identifier is \
             correct and that the resource exists."
        ),
    }
}

/// The operating policy and sources a code environment runs with, as
/// `CreateEnvironmentConfiguration` / `UpdateEnvironmentConfiguration` carry it
/// and `EnvironmentConfigurationDetails` reports it back.
#[derive(Debug, Clone, Default)]
pub struct EnvironmentConfig {
    pub code_configuration: Option<CodeConfig>,
}

/// A code environment's stored sources and operating policy.
#[derive(Debug, Clone, Default)]
pub struct CodeConfig {
    pub source_configuration: Option<EnvironmentSourceConfiguration>,
    pub environment_configuration: Option<EnvironmentConfiguration>,
}

impl EnvironmentConfig {
    /// The configuration a create persists.
    ///
    /// `CreateEnvironmentConfiguration` documents that omission "creates a code
    /// environment with the default operating policy and no sources", so an
    /// omitted member — the whole configuration, its `codeConfiguration`, or one
    /// policy member of it — leaves the documented default in the store rather
    /// than nothing.
    fn from_create(requested: Option<CreateEnvironmentConfiguration>) -> Self {
        let requested_code = requested.and_then(|configuration| configuration.code_configuration);
        let (sources, policy) = match requested_code {
            Some(code) => (code.source_configuration, code.environment_configuration),
            None => (None, None),
        };
        let policy = policy.unwrap_or_default();
        Self {
            code_configuration: Some(CodeConfig {
                source_configuration: sources,
                environment_configuration: Some(EnvironmentConfiguration {
                    network: policy.network,
                    scan_frequency: Some(
                        policy
                            .scan_frequency
                            .unwrap_or_else(|| DEFAULT_SCAN_FREQUENCY.to_string()),
                    ),
                    patch_creation_policy: Some(
                        policy
                            .patch_creation_policy
                            .unwrap_or_else(|| DEFAULT_PATCH_CREATION_POLICY.to_string()),
                    ),
                }),
            }),
        }
    }

    /// Merge the changes an update carries into the stored configuration.
    ///
    /// `UpdateEnvironmentConfiguration` documents the rule at every level:
    /// "Omitted members preserve stored values", and a member the request does
    /// carry replaces the stored one — `s3Sources` replaces the environment's S3
    /// source, `repositorySources` its repository selections, `network` its
    /// network policy, `patchCreationPolicy` and `scanFrequency` their policies.
    fn apply_update(&mut self, requested: UpdateEnvironmentConfiguration) {
        let Some(code) = requested.code_configuration else {
            return;
        };
        let stored = self.code_configuration.get_or_insert_with(CodeConfig::default);
        if let Some(sources) = code.source_configuration {
            let stored_sources = stored
                .source_configuration
                .get_or_insert_with(EnvironmentSourceConfiguration::default);
            if sources.s3_sources.is_some() {
                stored_sources.s3_sources = sources.s3_sources;
            }
            if sources.repository_sources.is_some() {
                stored_sources.repository_sources = sources.repository_sources;
            }
        }
        if let Some(policy) = code.environment_configuration {
            let stored_policy = stored
                .environment_configuration
                .get_or_insert_with(EnvironmentConfiguration::default);
            if policy.network.is_some() {
                stored_policy.network = policy.network;
            }
            if policy.scan_frequency.is_some() {
                stored_policy.scan_frequency = policy.scan_frequency;
            }
            if policy.patch_creation_policy.is_some() {
                stored_policy.patch_creation_policy = policy.patch_creation_policy;
            }
        }
    }

    /// The stored configuration as `EnvironmentConfigurationDetails`, the shape
    /// `GetEnvironment` and `UpdateEnvironment` report it in.
    fn details(&self) -> EnvironmentConfigurationDetails {
        EnvironmentConfigurationDetails {
            code_configuration: self.code_configuration.as_ref().map(|code| {
                CodeConfigurationDetails {
                    // Both members report other resources of this service: the
                    // collector `AssociateCollector` attached, and the run
                    // `CreateCodeScan` last started. Neither operation is
                    // implemented, so no stored collector or scan backs either
                    // one and reporting a value here would describe work
                    // nothing performed.
                    collector_details: None,
                    latest_scan: None,
                    source_configuration: code.source_configuration.clone(),
                    environment_configuration: code.environment_configuration.clone(),
                }
            }),
        }
    }
}

/// One stored environment.
#[derive(Debug, Clone)]
pub struct Environment {
    pub environment_id: String,
    pub environment_arn: String,
    pub name: String,
    pub status: String,
    pub status_message: Option<String>,
    pub status_updated_at: SystemTime,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub configuration: EnvironmentConfig,
}

impl Environment {
    /// The record a create persists: the id it was minted under, the ARN that
    /// id resolves to in the region the request was signed for, the name the
    /// caller gave, and the configuration [`EnvironmentConfig::from_create`]
    /// resolved.
    pub fn new(
        region: &str,
        environment_id: String,
        name: String,
        configuration: Option<CreateEnvironmentConfiguration>,
        at: SystemTime,
    ) -> Self {
        Self {
            environment_arn: environment_arn(region, &environment_id),
            environment_id,
            name,
            status: ENVIRONMENT_STATUS_ACTIVE.to_string(),
            // Nothing has qualified this environment's status, so it carries no
            // message: `statusMessage` is optional and reports why a status is
            // what it is.
            status_message: None,
            status_updated_at: at,
            created_at: at,
            updated_at: at,
            configuration: EnvironmentConfig::from_create(configuration),
        }
    }

    /// Write the members an update carries, leaving every member it omits as it
    /// was stored, and record that the environment changed at `at`.
    pub fn apply_update(
        &mut self,
        name: Option<String>,
        configuration: Option<UpdateEnvironmentConfiguration>,
        at: SystemTime,
    ) {
        if let Some(name) = name {
            self.name = name;
        }
        if let Some(configuration) = configuration {
            self.configuration.apply_update(configuration);
        }
        self.updated_at = at;
        // The environment is re-validated under its new configuration, so the
        // status is restated as of this call even though ACTIVE is the only
        // status it can hold.
        self.status_updated_at = at;
    }

    /// `CreateEnvironment`'s output: the four members the model declares.
    pub fn create_output(&self) -> CreateEnvironmentOutput {
        CreateEnvironmentOutput {
            environment_id: self.environment_id.clone(),
            environment_arn: self.environment_arn.clone(),
            status: self.status.clone(),
            created_at: timestamp(self.created_at),
        }
    }

    /// `GetEnvironment`'s output: the whole stored record.
    pub fn get_output(&self) -> GetEnvironmentOutput {
        GetEnvironmentOutput {
            environment_id: self.environment_id.clone(),
            environment_arn: self.environment_arn.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            status_updated_at: timestamp(self.status_updated_at),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            configuration: self.configuration.details(),
        }
    }

    /// `UpdateEnvironment`'s output: the merged record. The model gives it no
    /// `updatedAt`, so the instant this call wrote is observed through
    /// `GetEnvironment`.
    pub fn update_output(&self) -> UpdateEnvironmentOutput {
        UpdateEnvironmentOutput {
            environment_id: self.environment_id.clone(),
            environment_arn: self.environment_arn.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            status_updated_at: timestamp(self.status_updated_at),
            created_at: timestamp(self.created_at),
            configuration: self.configuration.details(),
        }
    }

    /// One `ListEnvironments` entry: the five members `EnvironmentListEntry`
    /// declares, every one read out of this record.
    pub fn list_entry(&self) -> EnvironmentListEntry {
        EnvironmentListEntry {
            environment_id: self.environment_id.clone(),
            environment_arn: self.environment_arn.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            created_at: timestamp(self.created_at),
        }
    }
}

impl ComAmazonawsContinuumContinuumcontrolplaneState {
    /// Mint an `environmentId` no stored environment holds.
    ///
    /// The id is derived from the name, the store's own monotonic counter and
    /// the creation instant, so two creates — of the same name or of different
    /// ones — cannot collide, and the loop re-mints rather than overwriting if
    /// one ever did.
    pub fn mint_environment_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.next_environment_seq = self.next_environment_seq.wrapping_add(1);
            let candidate = derived_id(name, self.next_environment_seq, at);
            if !self.environments.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// One page of the live environments, in `environmentId` order, resuming
    /// after the key a previous page ended on.
    ///
    /// The second member is the `nextToken` for the page after this one, and is
    /// present only while entries remain — a page that exhausted the store ends
    /// the listing.
    pub fn page_environments(
        &self,
        after: Option<&str>,
        page_size: usize,
    ) -> (Vec<EnvironmentListEntry>, Option<String>) {
        let lower = match after {
            Some(key) => Bound::Excluded(key),
            None => Bound::Unbounded,
        };
        let mut items = Vec::new();
        let mut next_token = None;
        let mut last_included: Option<&str> = None;
        for (key, record) in self
            .environments
            .range::<str, _>((lower, Bound::Unbounded))
        {
            if items.len() >= page_size {
                // At least one environment is left over, so hand back a token
                // that resumes exactly where this page ended.
                next_token = last_included
                    .map(|cursor| encode_page_token(&self.page_token_key, cursor));
                break;
            }
            items.push(record.list_entry());
            last_included = Some(key.as_str());
        }
        (items, next_token)
    }
}

/// An `^[a-z0-9]+$` identifier derived from a name, a mint counter and an
/// instant: the workspace's one stable mixing function
/// ([`service_common::stable_hex_id`], SHA-256 over the parts), whose hex output
/// is a subset of the characters `EnvironmentId`'s pattern admits.
fn derived_id(name: &str, counter: u64, at: SystemTime) -> String {
    let nanos = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    ::service_common::stable_hex_id(&[name, &counter.to_string(), &nanos.to_string()])
}
