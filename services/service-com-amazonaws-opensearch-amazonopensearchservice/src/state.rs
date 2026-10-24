// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-opensearch-amazonopensearchservice service.
//!
//! The domain is the primary resource of the OpenSearch Service control plane:
//! `CreateDomain` mints one, `DescribeDomain` / `DescribeDomainConfig` /
//! `ListDomainNames` report it, `UpdateDomainConfig` mutates it and `DeleteDomain`
//! removes it. [`Domain`] below is the ONE copy of a domain every one of those
//! handlers reads and writes, plus the three projections the model asks for
//! (`DomainStatus`, `DomainConfig` and `DomainInfo`) — so no two operations can
//! report the same record differently.
//!
//! A few leaf values are stored as the modelled shapes from [`crate::ops::types`]
//! (`ClusterConfig`, `EBSOptions`, …) rather than as a second hand-written mirror
//! of the same members: the model already declares them, and two copies of one
//! shape is how two operations end up reading two different records.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AIMLOptionsInput, AIMLOptionsOutput, AIMLOptionsStatus, AccessPoliciesStatus,
    AdvancedOptionsStatus, AdvancedSecurityOptions, AdvancedSecurityOptionsInput,
    AdvancedSecurityOptionsStatus, AutoTuneOptions, AutoTuneOptionsInput, AutoTuneOptionsOutput,
    AutoTuneOptionsStatus, AutoTuneStatus, AutomatedSnapshotPauseOptions,
    AutomatedSnapshotPauseOptionsStatus, AutomatedSnapshotPauseRequestOptions,
    ChangeProgressDetails, ChangeProgressStatusDetails, ClusterConfig, ClusterConfigStatus,
    CognitoOptions, CognitoOptionsStatus, DataSourceDetails, DataSourceType,
    DeploymentStrategyOptions,
    DeploymentStrategyOptionsStatus, DomainConfig, DomainEndpointOptions,
    DomainEndpointOptionsStatus, DomainInfo, DomainStatus, EBSOptions, EBSOptionsStatus,
    EncryptionAtRestOptions, EncryptionAtRestOptionsStatus, EngineModeStatus,
    IAMFederationOptionsOutput, IPAddressTypeStatus, IdentityCenterOptions,
    IdentityCenterOptionsInput, IdentityCenterOptionsStatus, JWTOptionsOutput, LogPublishingOption,
    LogPublishingOptionsStatus, NaturalLanguageQueryGenerationOptionsOutput,
    NodeToNodeEncryptionOptions, NodeToNodeEncryptionOptionsStatus, OffPeakWindowOptions,
    OffPeakWindowOptionsStatus, OptionStatus, ResourceNotFoundException, SAMLOptionsOutput,
    SnapshotOptions, SnapshotOptionsStatus, SoftwareUpdateOptions, SoftwareUpdateOptionsStatus,
    Tag, UseCaseStatus, VPCDerivedInfo, VPCDerivedInfoStatus, VPCOptions, ValidationException,
    VersionStatus,
};

/// The engine version a domain runs when `CreateDomain` names none.
///
/// `EngineVersion` is optional and the service picks a current OpenSearch version
/// when it is omitted. `ListVersions`, which publishes the supported set, is still
/// a stub, so this constant is the emulator's single statement of what that is. It
/// satisfies `VersionString`'s own pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`.
pub const DEFAULT_ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The use case a domain is created with when the request names none.
///
/// From `DomainUseCase`'s own documentation: "If you don't specify a use case,
/// `MIXED` is used."
pub const DEFAULT_USE_CASE: &str = "MIXED";

/// The engine mode a domain is created with when the request names none.
///
/// From `EngineMode`'s own documentation: "If you don't specify an engine mode,
/// `GENERAL` is used."
pub const DEFAULT_ENGINE_MODE: &str = "GENERAL";

/// The region a domain's ARN and endpoint are built for when the request carries no
/// SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace and endpoint prefix of this service, from the model's
/// `aws.api#service` trait (`arnNamespace`/`endpointPrefix`: `es`).
pub const ARN_NAMESPACE: &str = "es";

/// How many domains the store holds before `CreateDomain` refuses with the modelled
/// `LimitExceededException`.
///
/// This is the emulator's OWN bound on the store — it models neither Service Quotas
/// nor an account's adjustments — set at the value OpenSearch Service publishes as
/// its default domain-per-Region quota. It is what stops the state growing without
/// end: every record it holds is released by `DeleteDomain`.
pub const DOMAIN_QUOTA: usize = 100;

/// How many data sources ONE domain holds before `AddDataSource` refuses with the
/// modelled `LimitExceededException`.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it models
/// neither Service Quotas nor an account's adjustments — and it is what stops a
/// caller looping with fresh names from growing the state without end: every record
/// it counts is released by `DeleteDataSource`. Because the per-domain set IS what
/// `ListDataSources` answers with, it also bounds the size of that answer, and with
/// [`DOMAIN_QUOTA`] it bounds the whole data-source store.
pub const DATA_SOURCE_QUOTA: usize = 20;

/// How long a domain stays `Processing` after the call that started the work
/// (`CreateDomain`, or an `UpdateDomainConfig` that changed something) before the
/// clock owes it a settled, `Active` configuration.
///
/// OpenSearch Service takes many minutes; the emulator chooses a span, and it is
/// deliberately NOT zero so a caller acting straight after the call still observes
/// the transient `Processing` state and the `Processing` option states that go with
/// it. The due instant is stored on the record and materialised by whichever later
/// call reads it — nothing sleeps.
pub const PROVISIONING: Duration = Duration::from_millis(500);

/// `EngineType`'s `OpenSearch` value, and the `EngineVersion` prefix that implies it.
pub const ENGINE_TYPE_OPENSEARCH: &str = "OpenSearch";

/// `EngineType`'s `Elasticsearch` value, and the `EngineVersion` prefix that implies
/// it.
pub const ENGINE_TYPE_ELASTICSEARCH: &str = "Elasticsearch";

/// `OptionState`'s `Processing` value: "The requested change is still in progress."
pub const OPTION_STATE_PROCESSING: &str = "Processing";

/// `OptionState`'s `Active` value: the requested change is live.
pub const OPTION_STATE_ACTIVE: &str = "Active";

/// `DomainProcessingStatusType`'s `Creating`.
pub const PROCESSING_STATUS_CREATING: &str = "Creating";

/// `DomainProcessingStatusType`'s `Modifying`.
pub const PROCESSING_STATUS_MODIFYING: &str = "Modifying";

/// `DomainProcessingStatusType`'s `Active`.
pub const PROCESSING_STATUS_ACTIVE: &str = "Active";

/// `DomainProcessingStatusType`'s `Deleting`.
pub const PROCESSING_STATUS_DELETING: &str = "Deleting";

/// `OverallChangeStatus`'s `PROCESSING` value: the configuration change is being
/// applied to the domain.
pub const CHANGE_STATUS_PROCESSING: &str = "PROCESSING";

/// `OverallChangeStatus`'s `COMPLETED` value: the configuration change has landed.
pub const CHANGE_STATUS_COMPLETED: &str = "COMPLETED";

/// `ConfigChangeStatus`'s `ApplyingChanges` value.
pub const CONFIG_CHANGE_APPLYING_CHANGES: &str = "ApplyingChanges";

/// `ConfigChangeStatus`'s `Completed` value.
pub const CONFIG_CHANGE_COMPLETED: &str = "Completed";

/// `InitiatedBy`'s `CUSTOMER` value: every change this emulator records was started
/// by a caller's own `CreateDomain` or `UpdateDomainConfig` request, never by the
/// service itself (a service software update, which would be `SERVICE`, is still a
/// stub).
pub const INITIATED_BY_CUSTOMER: &str = "CUSTOMER";

/// How many configuration changes a domain's history keeps.
///
/// This is the emulator's OWN bound on the store, not a published AWS limit: it is
/// what stops a domain's change history growing without end under a caller that
/// updates it forever. The oldest change is dropped first, so the most recent
/// changes — the ones `DescribeDomainChangeProgress` is asked about — are the ones
/// retained.
pub const CHANGE_HISTORY: usize = 50;

// ---------------------------------------------------------------------------
// Option names
//
// A domain's configuration is versioned PER OPTION: `DomainConfig` reports every
// option wrapped in a `*Status` container carrying an `OptionStatus`, whose
// `UpdateVersion`/`UpdateDate` are that option's own history. The constants below
// are the member names `DomainConfig` reports each option under, so the key a
// record versions an option by is the same name the wire uses.
// ---------------------------------------------------------------------------

/// `DomainConfig$EngineVersion`.
pub const OPT_ENGINE_VERSION: &str = "EngineVersion";
/// `DomainConfig$ClusterConfig`.
pub const OPT_CLUSTER_CONFIG: &str = "ClusterConfig";
/// `DomainConfig$EBSOptions`.
pub const OPT_EBS_OPTIONS: &str = "EBSOptions";
/// `DomainConfig$AccessPolicies`.
pub const OPT_ACCESS_POLICIES: &str = "AccessPolicies";
/// `DomainConfig$IPAddressType`.
pub const OPT_IP_ADDRESS_TYPE: &str = "IPAddressType";
/// `DomainConfig$SnapshotOptions`.
pub const OPT_SNAPSHOT_OPTIONS: &str = "SnapshotOptions";
/// `DomainConfig$VPCOptions`.
pub const OPT_VPC_OPTIONS: &str = "VPCOptions";
/// `DomainConfig$CognitoOptions`.
pub const OPT_COGNITO_OPTIONS: &str = "CognitoOptions";
/// `DomainConfig$EncryptionAtRestOptions`.
pub const OPT_ENCRYPTION_AT_REST_OPTIONS: &str = "EncryptionAtRestOptions";
/// `DomainConfig$NodeToNodeEncryptionOptions`.
pub const OPT_NODE_TO_NODE_ENCRYPTION_OPTIONS: &str = "NodeToNodeEncryptionOptions";
/// `DomainConfig$AdvancedOptions`.
pub const OPT_ADVANCED_OPTIONS: &str = "AdvancedOptions";
/// `DomainConfig$LogPublishingOptions`.
pub const OPT_LOG_PUBLISHING_OPTIONS: &str = "LogPublishingOptions";
/// `DomainConfig$DomainEndpointOptions`.
pub const OPT_DOMAIN_ENDPOINT_OPTIONS: &str = "DomainEndpointOptions";
/// `DomainConfig$AdvancedSecurityOptions`.
pub const OPT_ADVANCED_SECURITY_OPTIONS: &str = "AdvancedSecurityOptions";
/// `DomainConfig$IdentityCenterOptions`.
pub const OPT_IDENTITY_CENTER_OPTIONS: &str = "IdentityCenterOptions";
/// `DomainConfig$AutoTuneOptions`.
pub const OPT_AUTO_TUNE_OPTIONS: &str = "AutoTuneOptions";
/// `DomainConfig$OffPeakWindowOptions`.
pub const OPT_OFF_PEAK_WINDOW_OPTIONS: &str = "OffPeakWindowOptions";
/// `DomainConfig$SoftwareUpdateOptions`.
pub const OPT_SOFTWARE_UPDATE_OPTIONS: &str = "SoftwareUpdateOptions";
/// `DomainConfig$AIMLOptions`.
pub const OPT_AIML_OPTIONS: &str = "AIMLOptions";
/// `DomainConfig$DeploymentStrategyOptions`.
pub const OPT_DEPLOYMENT_STRATEGY_OPTIONS: &str = "DeploymentStrategyOptions";
/// `DomainConfig$AutomatedSnapshotPauseOptions`.
pub const OPT_AUTOMATED_SNAPSHOT_PAUSE_OPTIONS: &str = "AutomatedSnapshotPauseOptions";
/// `DomainConfig$UseCase`.
pub const OPT_USE_CASE: &str = "UseCase";
/// `DomainConfig$EngineMode`.
pub const OPT_ENGINE_MODE: &str = "EngineMode";

/// One option's own revision history: the version it is at and when it last changed.
///
/// This is what lets `DescribeDomainConfig` report a truthful `OptionStatus` —
/// `UpdateVersion` and `UpdateDate` per option — instead of one constant for the
/// whole domain.
#[derive(Debug, Clone)]
pub struct OptionRevision {
    /// `OptionStatus$UpdateVersion`: `1` when the domain was created with this
    /// option, incremented by each later `UpdateDomainConfig` that changed it.
    pub version: i32,
    /// `OptionStatus$UpdateDate`: the instant of the call that last set it.
    pub updated_at: SystemTime,
}

/// `DataSourceStatus`'s `ACTIVE` value — the status a direct-query data source has
/// while queries may run against it. `DataSourceStatus` declares exactly
/// `["ACTIVE", "DISABLED"]`.
pub const DATA_SOURCE_STATUS_ACTIVE: &str = "ACTIVE";

/// `DataSourceStatus`'s `DISABLED` value — the other member of that same two-value
/// enum: the data source exists but queries may not run against it.
pub const DATA_SOURCE_STATUS_DISABLED: &str = "DISABLED";

/// Every value `DataSourceStatus` declares, in the model's own order. This is the
/// whole catalog a request may name and a report may carry.
pub const DATA_SOURCE_STATUSES: [&str; 2] =
    [DATA_SOURCE_STATUS_ACTIVE, DATA_SOURCE_STATUS_DISABLED];

/// One direct-query data source of a domain, as this emulator persists it.
///
/// `AddDataSource` mints one, `GetDataSource` reports it and `ListDataSources`
/// reports every one of its domain's — all from THIS record, so no two of them can
/// report the same data source differently.
#[derive(Debug, Clone)]
pub struct DataSource {
    /// `DataSourceDetails$Name`. Unique within the domain, so the name IS the key
    /// the domain's map holds it under.
    pub name: String,
    /// `DataSourceDetails$DataSourceType`: the `DataSourceType` union the create
    /// call carried, stored as the modelled shape rather than a second mirror of it.
    pub data_source_type: DataSourceType,
    /// `DataSourceDetails$Description`, absent when the create named none.
    pub description: Option<String>,
    /// `DataSourceDetails$Status`: one of [`DATA_SOURCE_STATUSES`].
    pub status: String,
}

impl DataSource {
    /// This record as `DataSourceDetails` — the element shape `ListDataSources`
    /// reports, and member-for-member the projection `GetDataSource` answers with.
    pub fn to_details(&self) -> DataSourceDetails {
        DataSourceDetails {
            data_source_type: Some(self.data_source_type.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: Some(self.status.clone()),
        }
    }
}

/// One configuration change on a domain: what `DescribeDomainChangeProgress`
/// reports in full and what `DomainStatus`/`DomainConfig` summarise as a
/// `ChangeProgressDetails`.
///
/// A change is minted by the call that starts it (`CreateDomain`, or an
/// `UpdateDomainConfig` that changed at least one option) and completes when the
/// provisioning span the clock owes it has passed — the same instant the domain
/// itself settles, so the change's status and the domain's `Processing` flag can
/// never disagree.
#[derive(Debug, Clone)]
pub struct ConfigChange {
    /// `ChangeProgressStatusDetails$ChangeId`: the GUID this change is addressed by.
    pub change_id: String,
    /// The `DomainConfig` member names this change sets, as the `OPT_*` constants
    /// spell them. Reported as `PendingProperties` while the change is in flight and
    /// as `CompletedProperties` once it has landed.
    pub properties: Vec<String>,
    /// `StartTime`: the instant of the call that started the change.
    pub started_at: SystemTime,
    /// `LastUpdatedTime`: the instant this change's status last moved.
    pub last_updated_at: SystemTime,
    /// When the clock owes this change completion; `None` once it has completed.
    pub completes_at: Option<SystemTime>,
    /// One of `InitiatedBy`'s values; see [`INITIATED_BY_CUSTOMER`].
    pub initiated_by: String,
}

impl ConfigChange {
    /// Materialise the completion the clock owes this change, if `now` is past its
    /// due instant.
    ///
    /// The completion is dated at the DUE instant rather than at `now`, so the
    /// `LastUpdatedTime` a caller reads does not move with the moment it happened to
    /// look.
    fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completes_at else {
            return;
        };
        if now < due {
            return;
        }
        self.completes_at = None;
        self.last_updated_at = due;
    }

    /// Whether this change is still being applied.
    pub fn in_progress(&self) -> bool {
        self.completes_at.is_some()
    }

    /// `ChangeProgressStatusDetails$Status`: one of `OverallChangeStatus`'s values.
    pub fn status(&self) -> &'static str {
        if self.in_progress() {
            CHANGE_STATUS_PROCESSING
        } else {
            CHANGE_STATUS_COMPLETED
        }
    }

    /// `ConfigChangeStatus` for this change: the emulator applies a change in one
    /// step, so it is `ApplyingChanges` until the change falls due and `Completed`
    /// after.
    pub fn config_change_status(&self) -> &'static str {
        if self.in_progress() {
            CONFIG_CHANGE_APPLYING_CHANGES
        } else {
            CONFIG_CHANGE_COMPLETED
        }
    }

    /// This change as `ChangeProgressStatusDetails` — what
    /// `DescribeDomainChangeProgress` answers with.
    pub fn to_progress_status(&self) -> ChangeProgressStatusDetails {
        // A property of the change in flight is pending; one of a change that has
        // landed is completed. The two lists partition `properties`, so no property
        // is ever reported as both.
        let (pending, completed) = if self.in_progress() {
            (self.properties.clone(), Vec::new())
        } else {
            (Vec::new(), self.properties.clone())
        };
        ChangeProgressStatusDetails {
            change_id: Some(self.change_id.clone()),
            start_time: Some(datetime(self.started_at)),
            status: Some(self.status().to_string()),
            pending_properties: Some(pending),
            completed_properties: Some(completed),
            // OpenSearch Service's blue/green deployment stages are not published,
            // and this emulator applies a configuration change in ONE step rather
            // than walking a sequence of stages — so no stage list is invented and
            // the total stays at the member's own modelled default.
            total_number_of_stages: 0,
            change_progress_stages: None,
            last_updated_time: Some(datetime(self.last_updated_at)),
            config_change_status: Some(self.config_change_status().to_string()),
            initiated_by: Some(self.initiated_by.clone()),
            // Validation failures come from the pre-update validation check a verbose
            // dry run runs (`DescribeDryRunProgress`), which is still a stub, so
            // there are none to report.
            validation_failures: None,
            // Advisory warnings are accepted through the same verbose dry run.
            accepted_warnings: None,
        }
    }

    /// This change as the `ChangeProgressDetails` every full descriptor of the domain
    /// (`DomainStatus`, `DomainConfig`) carries, so the change a descriptor names is
    /// the same change `DescribeDomainChangeProgress` reports.
    pub fn to_details(&self) -> ChangeProgressDetails {
        ChangeProgressDetails {
            change_id: Some(self.change_id.clone()),
            // `Message` is "a message corresponding to the status of the
            // configuration change"; the service's own wording for one is not
            // published, and the status itself is reported in
            // `ConfigChangeStatus`, so none is invented.
            message: None,
            config_change_status: Some(self.config_change_status().to_string()),
            initiated_by: Some(self.initiated_by.clone()),
            start_time: Some(datetime(self.started_at)),
            last_updated_time: Some(datetime(self.last_updated_at)),
        }
    }
}

/// One OpenSearch Service domain, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Domain {
    /// `DomainStatus$DomainId`: `<account-id>/<domain-name>`.
    pub domain_id: String,
    pub domain_name: String,
    /// `arn:aws:es:<region>:<account-id>:domain/<domain-name>`.
    pub arn: String,
    /// The Region the domain was created in — the one its ARN and endpoint name, and
    /// the one whose Availability Zones its nodes sit in (see
    /// [`crate::ops::describe_domain_health`]).
    pub region: String,
    pub engine_version: String,
    /// One of `EngineMode`'s values; see [`DEFAULT_ENGINE_MODE`].
    pub engine_mode: String,
    /// One of `DomainUseCase`'s values; see [`DEFAULT_USE_CASE`].
    pub use_case: String,
    /// The domain-specific endpoint index and search requests go to.
    pub endpoint: String,
    /// `DomainStatus$Created`.
    pub created: bool,
    /// `DomainStatus$Deleted`. Only ever true on the record `DeleteDomain` returns:
    /// the store no longer holds a deleted domain.
    pub deleted: bool,
    /// `DomainStatus$Processing`: a configuration change is in flight.
    pub processing: bool,
    /// One of `DomainProcessingStatusType`'s values.
    pub processing_status: String,
    /// When the clock owes this domain a settled configuration; see [`PROVISIONING`].
    pub processing_until: Option<SystemTime>,
    pub cluster_config: ClusterConfig,
    pub ebs_options: Option<EBSOptions>,
    pub access_policies: Option<String>,
    pub ip_address_type: Option<String>,
    pub snapshot_options: Option<SnapshotOptions>,
    pub vpc_options: Option<VPCDerivedInfo>,
    pub cognito_options: Option<CognitoOptions>,
    pub encryption_at_rest_options: Option<EncryptionAtRestOptions>,
    pub node_to_node_encryption_options: Option<NodeToNodeEncryptionOptions>,
    pub advanced_options: BTreeMap<String, String>,
    /// Keyed by a `LogType` value.
    pub log_publishing_options: BTreeMap<String, LogPublishingOption>,
    pub domain_endpoint_options: Option<DomainEndpointOptions>,
    pub advanced_security_options: Option<AdvancedSecurityOptions>,
    pub identity_center_options: Option<IdentityCenterOptions>,
    /// Stored in the `DomainConfig` form; `DomainStatus` reports the
    /// `AutoTuneOptionsOutput` projection of it.
    pub auto_tune_options: Option<AutoTuneOptions>,
    pub off_peak_window_options: Option<OffPeakWindowOptions>,
    pub software_update_options: Option<SoftwareUpdateOptions>,
    pub aiml_options: Option<AIMLOptionsOutput>,
    pub deployment_strategy_options: Option<DeploymentStrategyOptions>,
    pub automated_snapshot_pause_options: Option<AutomatedSnapshotPauseOptions>,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// The instant of the last call that changed any option.
    pub updated_at: SystemTime,
    /// Per-option revision history, keyed by the `OPT_*` names above. An option with
    /// no entry has no persisted value, and `DomainConfig` reports no container for
    /// it.
    pub option_versions: BTreeMap<String, OptionRevision>,
    /// This domain's direct-query data sources, keyed by their `Name` — unique per
    /// domain, so the name IS the key. Iterating a `BTreeMap` is order-stable, which
    /// is what lets `ListDataSources` report the same order twice.
    pub data_sources: BTreeMap<String, DataSource>,
    /// This domain's configuration changes, oldest first, bounded by
    /// [`CHANGE_HISTORY`]: a FIFO, because the cap drops the oldest change and the
    /// newest is the one `DescribeDomainChangeProgress` reports when the request
    /// names no `ChangeId`.
    pub changes: VecDeque<ConfigChange>,
}

impl Domain {
    /// Record that `option` was set by a call at `now`: mint its first revision, or
    /// bump the one it has.
    ///
    /// Also moves the domain's own `updated_at` to `now`, which is what
    /// [`Self::option_state`] reads to tell an option touched by the change in
    /// flight from one that has been live since an earlier call.
    pub fn touch(&mut self, option: &str, now: SystemTime) {
        let revision = self
            .option_versions
            .entry(option.to_string())
            .or_insert(OptionRevision {
                version: 0,
                updated_at: now,
            });
        revision.version = revision.version.saturating_add(1);
        revision.updated_at = now;
        self.updated_at = now;
    }

    /// Put the domain into `Processing` for the change a call at `now` started: store
    /// the instant the clock owes it a settled configuration, and RECORD the change
    /// — the options it is applying — so `DescribeDomainChangeProgress` can report on
    /// it.
    ///
    /// `properties` are the `OPT_*` names the call set, which are exactly the ones it
    /// [`Self::touch`]ed; they are taken from the caller rather than inferred from a
    /// timestamp, so two calls landing on one clock tick still record one change
    /// each, with its own properties. They are stored sorted and deduplicated, so the
    /// same change reports the same list twice.
    pub fn begin_change(&mut self, status: &str, properties: &[&str], now: SystemTime) {
        self.processing = true;
        self.processing_status = status.to_string();
        self.processing_until = Some(now + PROVISIONING);
        let mut properties: Vec<String> =
            properties.iter().map(|option| option.to_string()).collect();
        properties.sort();
        properties.dedup();
        self.changes.push_back(ConfigChange {
            change_id: new_change_id(),
            properties,
            started_at: now,
            last_updated_at: now,
            completes_at: self.processing_until,
            initiated_by: INITIATED_BY_CUSTOMER.to_string(),
        });
        // The history is bounded: past its cap the oldest change is dropped, so a
        // domain updated forever holds a fixed number of records.
        while self.changes.len() > CHANGE_HISTORY {
            self.changes.pop_front();
        }
    }

    /// The most recent configuration change, which every full descriptor summarises
    /// and `DescribeDomainChangeProgress` reports when the request names no
    /// `ChangeId`.
    pub fn latest_change(&self) -> Option<&ConfigChange> {
        self.changes.back()
    }

    /// The change `change_id` names, or `None` when this domain's history holds no
    /// such change.
    pub fn change(&self, change_id: &str) -> Option<&ConfigChange> {
        self.changes
            .iter()
            .find(|change| change.change_id == change_id)
    }

    /// Materialise the configuration change the clock owes this domain, if `now` is
    /// past its due instant: the change lands, so the domain stops `Processing` and
    /// its options go `Active`.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        for change in &mut self.changes {
            change.settle(now);
        }
        let Some(due) = self.processing_until else {
            return;
        };
        if now < due {
            return;
        }
        self.processing = false;
        self.processing_status = PROCESSING_STATUS_ACTIVE.to_string();
        self.processing_until = None;
    }

    /// `EngineType` for this domain, derived from its `EngineVersion` prefix.
    pub fn engine_type(&self) -> Option<&'static str> {
        engine_type_of(&self.engine_version)
    }

    /// The `OptionState` of one option: `Processing` while the domain is applying a
    /// change this option was part of, `Active` otherwise.
    fn option_state(&self, revision: &OptionRevision) -> &'static str {
        if self.processing && revision.updated_at == self.updated_at {
            OPTION_STATE_PROCESSING
        } else {
            OPTION_STATE_ACTIVE
        }
    }

    /// The `OptionStatus` for one option, or `None` when the domain holds no value
    /// for it.
    pub fn option_status(&self, option: &str) -> Option<OptionStatus> {
        let revision = self.option_versions.get(option)?;
        Some(OptionStatus {
            creation_date: datetime(self.created_at),
            update_date: datetime(revision.updated_at),
            update_version: revision.version,
            state: self.option_state(revision).to_string(),
            pending_deletion: Some(self.deleted),
        })
    }

    /// This domain as `DomainStatus` — what `CreateDomain`, `DescribeDomain` and
    /// `DeleteDomain` answer with.
    pub fn to_status(&self) -> DomainStatus {
        DomainStatus {
            domain_id: self.domain_id.clone(),
            domain_name: self.domain_name.clone(),
            arn: self.arn.clone(),
            created: Some(self.created),
            deleted: Some(self.deleted),
            endpoint: Some(self.endpoint.clone()),
            // A version 2 endpoint and a dual-stack hosted zone are provisioned by
            // the parts of the service this slice does not model.
            endpoint_v2: None,
            // The `Endpoints` map exists only for a VPC domain's endpoints, which
            // `CreateVpcEndpoint` would mint; it is still a stub.
            endpoints: None,
            domain_endpoint_v2_hosted_zone_id: None,
            processing: Some(self.processing),
            // No version upgrade is in flight: `UpgradeDomain` is still a stub.
            upgrade_processing: Some(false),
            engine_version: Some(self.engine_version.clone()),
            cluster_config: self.cluster_config.clone(),
            ebs_options: self.ebs_options.clone(),
            access_policies: self.access_policies.clone(),
            ip_address_type: self.ip_address_type.clone(),
            snapshot_options: self.snapshot_options.clone(),
            vpc_options: self.vpc_options.clone(),
            cognito_options: self.cognito_options.clone(),
            encryption_at_rest_options: self.encryption_at_rest_options.clone(),
            node_to_node_encryption_options: self.node_to_node_encryption_options.clone(),
            advanced_options: Some(
                self.advanced_options
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            log_publishing_options: Some(
                self.log_publishing_options
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            // The service software a domain runs, and whether an update is
            // available, is reported by the service-software operations — all still
            // stubs, so there is no record to report.
            service_software_options: None,
            domain_endpoint_options: self.domain_endpoint_options.clone(),
            advanced_security_options: self.advanced_security_options.clone(),
            identity_center_options: self.identity_center_options.clone(),
            auto_tune_options: self.auto_tune_options.as_ref().map(|options| {
                AutoTuneOptionsOutput {
                    state: options.desired_state.clone(),
                    error_message: None,
                    use_off_peak_window: options.use_off_peak_window,
                }
            }),
            // The change this descriptor reports is the same record
            // `DescribeDomainChangeProgress` answers about, read through its own
            // projection.
            change_progress_details: self.latest_change().map(ConfigChange::to_details),
            off_peak_window_options: self.off_peak_window_options.clone(),
            software_update_options: self.software_update_options.clone(),
            domain_processing_status: Some(self.processing_status.clone()),
            // Nothing is reported as modifying: this emulator applies an update in
            // one step and then settles it.
            modifying_properties: None,
            aiml_options: self.aiml_options.clone(),
            deployment_strategy_options: self.deployment_strategy_options.clone(),
            automated_snapshot_pause_options: self.automated_snapshot_pause_options.clone(),
            use_case: Some(self.use_case.clone()),
            engine_mode: Some(self.engine_mode.clone()),
        }
    }

    /// This domain as `DomainConfig` — what `DescribeDomainConfig` and
    /// `UpdateDomainConfig` answer with. Every option the domain holds a value for
    /// is reported wrapped in its own `*Status`, carrying that option's
    /// [`Self::option_status`].
    pub fn to_config(&self) -> DomainConfig {
        DomainConfig {
            engine_version: self
                .option_status(OPT_ENGINE_VERSION)
                .map(|status| VersionStatus {
                    options: self.engine_version.clone(),
                    status,
                }),
            cluster_config: self
                .option_status(OPT_CLUSTER_CONFIG)
                .map(|status| ClusterConfigStatus {
                    options: self.cluster_config.clone(),
                    status,
                }),
            ebs_options: self
                .ebs_options
                .clone()
                .zip(self.option_status(OPT_EBS_OPTIONS))
                .map(|(options, status)| EBSOptionsStatus { options, status }),
            access_policies: self
                .access_policies
                .clone()
                .zip(self.option_status(OPT_ACCESS_POLICIES))
                .map(|(options, status)| AccessPoliciesStatus { options, status }),
            ip_address_type: self
                .ip_address_type
                .clone()
                .zip(self.option_status(OPT_IP_ADDRESS_TYPE))
                .map(|(options, status)| IPAddressTypeStatus { options, status }),
            snapshot_options: self
                .snapshot_options
                .clone()
                .zip(self.option_status(OPT_SNAPSHOT_OPTIONS))
                .map(|(options, status)| SnapshotOptionsStatus { options, status }),
            vpc_options: self
                .vpc_options
                .clone()
                .zip(self.option_status(OPT_VPC_OPTIONS))
                .map(|(options, status)| VPCDerivedInfoStatus { options, status }),
            cognito_options: self
                .cognito_options
                .clone()
                .zip(self.option_status(OPT_COGNITO_OPTIONS))
                .map(|(options, status)| CognitoOptionsStatus { options, status }),
            encryption_at_rest_options: self
                .encryption_at_rest_options
                .clone()
                .zip(self.option_status(OPT_ENCRYPTION_AT_REST_OPTIONS))
                .map(|(options, status)| EncryptionAtRestOptionsStatus { options, status }),
            node_to_node_encryption_options: self
                .node_to_node_encryption_options
                .clone()
                .zip(self.option_status(OPT_NODE_TO_NODE_ENCRYPTION_OPTIONS))
                .map(|(options, status)| NodeToNodeEncryptionOptionsStatus { options, status }),
            advanced_options: self.option_status(OPT_ADVANCED_OPTIONS).map(|status| {
                AdvancedOptionsStatus {
                    options: self
                        .advanced_options
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                    status,
                }
            }),
            log_publishing_options: self.option_status(OPT_LOG_PUBLISHING_OPTIONS).map(|status| {
                LogPublishingOptionsStatus {
                    options: Some(
                        self.log_publishing_options
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    ),
                    status: Some(status),
                }
            }),
            domain_endpoint_options: self
                .domain_endpoint_options
                .clone()
                .zip(self.option_status(OPT_DOMAIN_ENDPOINT_OPTIONS))
                .map(|(options, status)| DomainEndpointOptionsStatus { options, status }),
            advanced_security_options: self
                .advanced_security_options
                .clone()
                .zip(self.option_status(OPT_ADVANCED_SECURITY_OPTIONS))
                .map(|(options, status)| AdvancedSecurityOptionsStatus { options, status }),
            identity_center_options: self
                .identity_center_options
                .clone()
                .zip(self.option_status(OPT_IDENTITY_CENTER_OPTIONS))
                .map(|(options, status)| IdentityCenterOptionsStatus { options, status }),
            auto_tune_options: self.auto_tune_options.clone().zip(
                self.option_status(OPT_AUTO_TUNE_OPTIONS),
            ).map(|(options, status)| AutoTuneOptionsStatus {
                status: Some(AutoTuneStatus {
                    creation_date: status.creation_date,
                    update_date: status.update_date,
                    update_version: status.update_version,
                    // `AutoTuneStatus$State` is an `AutoTuneState`, not an
                    // `OptionState`: it reports whether Auto-Tune is on, which is
                    // the desired state the caller set.
                    state: options
                        .desired_state
                        .clone()
                        .unwrap_or_else(|| AUTO_TUNE_STATE_DISABLED.to_string()),
                    error_message: None,
                    pending_deletion: status.pending_deletion,
                }),
                options: Some(options),
            }),
            // The same record `DomainStatus` and `DescribeDomainChangeProgress`
            // report, so no two read paths name a different change.
            change_progress_details: self.latest_change().map(ConfigChange::to_details),
            off_peak_window_options: self.option_status(OPT_OFF_PEAK_WINDOW_OPTIONS).and_then(
                |status| {
                    self.off_peak_window_options
                        .clone()
                        .map(|options| OffPeakWindowOptionsStatus {
                            options: Some(options),
                            status: Some(status),
                        })
                },
            ),
            software_update_options: self.option_status(OPT_SOFTWARE_UPDATE_OPTIONS).and_then(
                |status| {
                    self.software_update_options
                        .clone()
                        .map(|options| SoftwareUpdateOptionsStatus {
                            options: Some(options),
                            status: Some(status),
                        })
                },
            ),
            modifying_properties: None,
            aiml_options: self
                .option_status(OPT_AIML_OPTIONS)
                .and_then(|status| {
                    self.aiml_options.clone().map(|options| AIMLOptionsStatus {
                        options: Some(options),
                        status: Some(status),
                    })
                }),
            deployment_strategy_options: self
                .deployment_strategy_options
                .clone()
                .zip(self.option_status(OPT_DEPLOYMENT_STRATEGY_OPTIONS))
                .map(|(options, status)| DeploymentStrategyOptionsStatus { options, status }),
            automated_snapshot_pause_options: self
                .automated_snapshot_pause_options
                .clone()
                .zip(self.option_status(OPT_AUTOMATED_SNAPSHOT_PAUSE_OPTIONS))
                .map(|(options, status)| AutomatedSnapshotPauseOptionsStatus { options, status }),
            use_case: self
                .option_status(OPT_USE_CASE)
                .map(|status| UseCaseStatus {
                    options: self.use_case.clone(),
                    status,
                }),
            engine_mode: self
                .option_status(OPT_ENGINE_MODE)
                .map(|status| EngineModeStatus {
                    options: self.engine_mode.clone(),
                    status,
                }),
        }
    }

    /// This domain as `DomainInfo` — what `ListDomainNames` reports it as.
    pub fn to_info(&self) -> DomainInfo {
        DomainInfo {
            domain_name: Some(self.domain_name.clone()),
            engine_type: self.engine_type().map(str::to_string),
        }
    }
}

/// `AutoTuneState`'s `DISABLED` value: what Auto-Tune's state is when a domain
/// carries Auto-Tune options that name no desired state.
pub const AUTO_TUNE_STATE_DISABLED: &str = "DISABLED";

/// Per-service state for com-amazonaws-opensearch-amazonopensearchservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Every created domain, keyed by its name — unique per account and Region, so
    /// the name IS the key. Iterating a `BTreeMap` is order-stable, which is what
    /// lets `ListDomainNames` report the same order twice.
    pub domains: BTreeMap<String, Domain>,
    /// Drives [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_endpoint_suffix`].
    pub endpoint_suffix_seq: u64,
}

/// The characters a domain endpoint's opaque suffix is built from.
const SUFFIX_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The length of a domain endpoint's opaque suffix, from `ServiceUrl`'s own example
/// `search-imdb-movies-oopcnjfn6ugo.eu-west-1.es.amazonaws.com`.
const SUFFIX_LEN: u32 = 12;

/// The number of distinct suffixes [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_endpoint_suffix`] can
/// render: `36^12`.
const SUFFIX_SPACE: u128 = 36u128.pow(SUFFIX_LEN);

/// Scrambles consecutive seeds across [`SUFFIX_SPACE`]. Prime, so it is coprime with
/// `36^12 = 2^24 * 3^24`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod
/// SUFFIX_SPACE)` a BIJECTION: distinct seeds can never mint the same suffix.
const SUFFIX_MULTIPLIER: u128 = 1_000_000_007;

/// The bijection's constant term, so the first suffix minted is not a run of leading
/// `a`s.
const SUFFIX_OFFSET: u128 = 734_291;

impl ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Mint the opaque suffix a domain's endpoint host carries, so two domains of
    /// the same name in different runs do not share an endpoint.
    ///
    /// Derived from a counter through the bijection described on
    /// [`SUFFIX_MULTIPLIER`], so it is unpredictable-looking yet never repeats.
    pub fn mint_endpoint_suffix(&mut self) -> String {
        self.endpoint_suffix_seq = self.endpoint_suffix_seq.wrapping_add(1);
        let mut scrambled =
            (u128::from(self.endpoint_suffix_seq) * SUFFIX_MULTIPLIER + SUFFIX_OFFSET)
                % SUFFIX_SPACE;
        let mut suffix = Vec::with_capacity(SUFFIX_LEN as usize);
        for _ in 0..SUFFIX_LEN {
            let digit = usize::try_from(scrambled % 36).unwrap_or(0);
            suffix.push(SUFFIX_ALPHABET[digit]);
            scrambled /= 36;
        }
        String::from_utf8(suffix).unwrap_or_default()
    }

    /// The domain `name` names, with any configuration change the clock owes it
    /// already materialised.
    pub fn settled_mut(&mut self, name: &str, now: SystemTime) -> Option<&mut Domain> {
        let domain = self.domains.get_mut(name)?;
        domain.settle(now);
        Some(domain)
    }

    /// Materialise every configuration change the clock owes, for a caller about to
    /// report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for domain in self.domains.values_mut() {
            domain.settle(now);
        }
    }
}

/// `EngineType` for an `EngineVersion`, derived from its prefix: `OpenSearch_X.Y`
/// runs the OpenSearch engine, `Elasticsearch_X.Y` the legacy Elasticsearch one.
///
/// `None` only for a version matching neither prefix, which `VersionString`'s
/// pattern `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`
/// does not allow.
pub fn engine_type_of(engine_version: &str) -> Option<&'static str> {
    if engine_version.starts_with("OpenSearch_") {
        Some(ENGINE_TYPE_OPENSEARCH)
    } else if engine_version.starts_with("Elasticsearch_") {
        Some(ENGINE_TYPE_ELASTICSEARCH)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Input -> stored projections
//
// Several option containers have an INPUT shape a request carries and an OUTPUT
// shape the service reports. The record stores the output shape, so the
// conversion happens once, here, rather than once per operation that accepts the
// option.
// ---------------------------------------------------------------------------

/// `AdvancedSecurityOptionsInput` as the service reports it back.
///
/// `MasterUserOptions` is deliberately dropped: it carries the master user's name
/// and PASSWORD, and `AdvancedSecurityOptions` — the shape every read path reports
/// — declares no member for either. A credential a caller sent is never echoed.
pub fn stored_advanced_security_options(
    input: AdvancedSecurityOptionsInput,
) -> AdvancedSecurityOptions {
    AdvancedSecurityOptions {
        enabled: input.enabled,
        internal_user_database_enabled: input.internal_user_database_enabled,
        saml_options: input.saml_options.map(|saml| SAMLOptionsOutput {
            enabled: saml.enabled,
            idp: saml.idp,
            subject_key: saml.subject_key,
            roles_key: saml.roles_key,
            session_timeout_minutes: saml.session_timeout_minutes,
        }),
        jwt_options: input.jwt_options.map(|jwt| JWTOptionsOutput {
            enabled: jwt.enabled,
            subject_key: jwt.subject_key,
            roles_key: jwt.roles_key,
            jwks_url: jwt.jwks_url,
            public_key: jwt.public_key,
        }),
        iam_federation_options: input
            .iam_federation_options
            .map(|federation| IAMFederationOptionsOutput {
                enabled: federation.enabled,
                subject_key: federation.subject_key,
                roles_key: federation.roles_key,
            }),
        // The date anonymous auth was disabled is set by the call that disables it,
        // which this slice does not model.
        anonymous_auth_disable_date: None,
        anonymous_auth_enabled: input.anonymous_auth_enabled,
    }
}

/// `IdentityCenterOptionsInput` as the service reports it back.
///
/// The application ARN and identity store id are minted by IAM Identity Center
/// itself, which this emulator does not model, so they stay absent rather than
/// being invented.
pub fn stored_identity_center_options(input: IdentityCenterOptionsInput) -> IdentityCenterOptions {
    IdentityCenterOptions {
        enabled_api_access: input.enabled_api_access,
        identity_center_instance_arn: input.identity_center_instance_arn,
        identity_center_instance_region: input.identity_center_instance_region,
        subject_key: input.subject_key,
        roles_key: input.roles_key,
        identity_center_application_arn: None,
        identity_store_id: None,
    }
}

/// `VPCOptions` as the service reports it back (`VPCDerivedInfo`).
///
/// The VPC id and the availability zones are DERIVED from the subnets by EC2, which
/// this emulator does not model, so they stay absent rather than being invented.
pub fn stored_vpc_options(input: VPCOptions) -> VPCDerivedInfo {
    VPCDerivedInfo {
        vpc_id: None,
        subnet_ids: input.subnet_ids,
        availability_zones: None,
        security_group_ids: input.security_group_ids,
        egress_enabled: input.egress_enabled,
    }
}

/// `AIMLOptionsInput` as the service reports it back.
///
/// The emulator applies the change in one step, so a feature's current state is the
/// state the caller asked for.
pub fn stored_aiml_options(input: AIMLOptionsInput) -> AIMLOptionsOutput {
    AIMLOptionsOutput {
        natural_language_query_generation_options: input
            .natural_language_query_generation_options
            .map(|options| NaturalLanguageQueryGenerationOptionsOutput {
                current_state: options.desired_state.clone(),
                desired_state: options.desired_state,
            }),
        s3_vectors_engine: input.s3_vectors_engine,
        serverless_vector_acceleration: input.serverless_vector_acceleration,
    }
}

/// `AutomatedSnapshotPauseRequestOptions` as the service reports it back.
pub fn stored_automated_snapshot_pause_options(
    input: AutomatedSnapshotPauseRequestOptions,
) -> AutomatedSnapshotPauseOptions {
    AutomatedSnapshotPauseOptions {
        enabled: input.enabled,
        start_time: input.start_time,
        end_time: input.end_time,
        // The pause state is reported by the snapshot subsystem, which this slice
        // does not model.
        state: None,
    }
}

/// `AutoTuneOptionsInput` (what `CreateDomain` carries) as the `AutoTuneOptions`
/// the record stores and `DomainConfig` reports.
///
/// `RollbackOnDisable` has no member on the create input — only
/// `UpdateDomainConfig` can set it — so it stays absent.
pub fn stored_auto_tune_options(input: AutoTuneOptionsInput) -> AutoTuneOptions {
    AutoTuneOptions {
        desired_state: input.desired_state,
        rollback_on_disable: None,
        maintenance_schedules: input.maintenance_schedules,
        use_off_peak_window: input.use_off_peak_window,
    }
}

/// The GUID a configuration change is addressed by.
///
/// `GUID` is `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$` —
/// exactly a hyphenated UUID — so the id comes from the `uuid` crate's v4 generator
/// rather than from a hand-rolled layout, and no two changes share one.
fn new_change_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// `DomainStatus$DomainId`: the account the domain belongs to and its name.
pub fn domain_id(domain_name: &str) -> String {
    format!("{}/{domain_name}", ::engine::emulator_account_id())
}

/// A domain's ARN: `arn:aws:es:<region>:<account-id>:domain/<domain-name>`.
pub fn domain_arn(domain_name: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_name}"))
}

/// The domain-specific endpoint index, search and data-upload requests go to.
///
/// The shape is `ServiceUrl`'s own example,
/// `search-imdb-movies-oopcnjfn6ugo.eu-west-1.es.amazonaws.com`: the `search-`
/// prefix, the domain name, an opaque per-domain suffix, then the Region and this
/// service's endpoint prefix.
pub fn domain_endpoint(domain_name: &str, suffix: &str, region: &str) -> String {
    format!("search-{domain_name}-{suffix}.{region}.{ARN_NAMESPACE}.amazonaws.com")
}

/// The region this request was signed for — what a domain's ARN and endpoint name.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SystemTime` as the modelled `UpdateTimestamp` type.
pub fn datetime(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The `ResourceNotFoundException` payload for a name that names no stored domain.
pub fn domain_not_found(domain_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Domain not found: {domain_name}")),
    }
}

/// The `ValidationException` payload for a name that names no data source of a
/// domain the store DOES hold.
///
/// Resolving a data source name against an existing domain is input validation, not
/// a missing-resource lookup: the refusal names the value that could not be
/// resolved, so a caller tells it apart from [`domain_not_found`].
pub fn data_source_absent(name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("DataSource with name {name} doesn't exist.")),
    }
}

/// The `ValidationException` payload for a create naming a data source the domain
/// already holds — the mirror of [`data_source_absent`], so one refusal reads like
/// the other.
pub fn data_source_exists(name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("DataSource with name {name} already exists.")),
    }
}

/// The widths of a `GUID`'s five hexadecimal groups.
const GUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `value` is a `GUID`: `@length(min: 36, max: 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`.
///
/// The request boundary enforces the length bound but not the pattern, so an
/// operation that takes a `GUID` checks the shape itself rather than treating a
/// malformed id as an id that merely names nothing. It is a shape test rather than a
/// parse: `Uuid::parse_str` would accept the simple and URN forms this pattern
/// forbids.
pub fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == GUID_GROUP_WIDTHS.len()
        && groups.iter().zip(GUID_GROUP_WIDTHS).all(|(group, width)| {
            group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// The `ResourceNotFoundException` payload for a change id a domain's history holds
/// no change for, and for a domain that has no configuration change to report.
pub fn change_not_found(domain_name: &str, change_id: Option<&str>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(match change_id {
            Some(change_id) => {
                format!("Change not found for domain {domain_name}: {change_id}")
            }
            None => format!("No configuration change found for domain {domain_name}"),
        }),
    }
}

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's
/// 501 rather than a 5xx because there is nothing to retry: no engine of this
/// process will ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the com-amazonaws-opensearch-amazonopensearchservice emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOpensearchAmazonopensearchserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOpensearchAmazonopensearchserviceState>)
}
