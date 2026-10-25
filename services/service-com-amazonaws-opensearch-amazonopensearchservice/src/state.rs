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
//!
//! Another resource this store holds is the cross-cluster [`Connection`]:
//! `CreateOutboundConnection` mints one and `DescribeOutboundConnections` reports
//! it. One record carries both views of a connection — the source domain owner's
//! outbound one and the destination owner's inbound one — because they are one
//! resource sharing a `ConnectionId`.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AIMLOptionsInput, AIMLOptionsOutput, AIMLOptionsStatus, AWSDomainInformation,
    AccessPoliciesStatus,
    AdvancedOptionsStatus, AdvancedSecurityOptions, AdvancedSecurityOptionsInput,
    AdvancedSecurityOptionsStatus, AutoTuneOptions, AutoTuneOptionsInput, AutoTuneOptionsOutput,
    AutoTuneOptionsStatus, AutoTuneStatus, AutomatedSnapshotPauseOptions,
    AutomatedSnapshotPauseOptionsStatus, AutomatedSnapshotPauseRequestOptions,
    ChangeProgressDetails, ChangeProgressStatusDetails, ClusterConfig, ClusterConfigStatus,
    CognitoOptions, CognitoOptionsStatus, ConflictException, ConnectionProperties,
    DataSourceDetails, DataSourceType,
    DeploymentStrategyOptions,
    DeploymentStrategyOptionsStatus, DomainConfig, DomainEndpointOptions,
    DomainEndpointOptionsStatus, DomainInfo, DomainInformationContainer, DomainPackageDetails,
    DomainStatus, EBSOptions,
    EBSOptionsStatus,
    EncryptionAtRestOptions, EncryptionAtRestOptionsStatus, EngineModeStatus,
    IAMFederationOptionsOutput, IPAddressTypeStatus, IdentityCenterOptions,
    IdentityCenterOptionsInput, IdentityCenterOptionsStatus, InstanceTypeDetails,
    InvalidPaginationTokenException, JWTOptionsOutput, LogPublishingOption,
    LogPublishingOptionsStatus, NaturalLanguageQueryGenerationOptionsOutput,
    NodeToNodeEncryptionOptions, NodeToNodeEncryptionOptionsStatus, OffPeakWindowOptions,
    OffPeakWindowOptionsStatus, OptionStatus, OutboundConnection, OutboundConnectionStatus,
    PackageAssociationConfiguration, PackageConfiguration, PackageDetails,
    PackageEncryptionOptions, PackageSource,
    PackageVendingOptions,
    RecurringCharge, ReservedInstance,
    ReservedInstanceOffering, ResourceAlreadyExistsException, ResourceNotFoundException,
    RollbackServiceSoftwareOptions, SAMLOptionsOutput, ServiceSoftwareOptions, SnapshotOptions,
    SnapshotOptionsStatus, SoftwareUpdateOptions, SoftwareUpdateOptionsStatus, Tag, UseCaseStatus,
    VPCDerivedInfo, VPCDerivedInfoStatus, VPCOptions, ValidationException, VersionStatus,
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES, RESERVED_INSTANCE_PAYMENT_OPTION_VALUES,
};

/// Every engine version this emulator publishes as supported, in UPGRADE ORDER
/// (oldest first): the legacy Elasticsearch line, then the OpenSearch line that
/// succeeds it.
///
/// Which versions a Region supports is published BY the service, not created by a
/// caller, so this is a catalog rather than state an operation writes — the same
/// shape as [`seed_reserved_instance_offerings`]'s offering catalog. The Smithy model
/// carries no list of them (only `VersionString`'s own pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`, which
/// every entry below satisfies), so this is the emulator's OWN published set and the
/// ONE statement of it: what it guarantees is the RELATION between its entries —
/// both engines, several majors of each, and several minors of a major — so a caller
/// can tell an upgrade forwards from one that goes nowhere.
///
/// Order is the order of [`upgrade_rank`], and the list is iterated in place, which
/// is what lets two reads report the same versions in the same order.
pub const SUPPORTED_ENGINE_VERSIONS: [&str; 32] = [
    "Elasticsearch_1.5",
    "Elasticsearch_2.3",
    "Elasticsearch_5.1",
    "Elasticsearch_5.3",
    "Elasticsearch_5.5",
    "Elasticsearch_5.6",
    "Elasticsearch_6.0",
    "Elasticsearch_6.2",
    "Elasticsearch_6.3",
    "Elasticsearch_6.4",
    "Elasticsearch_6.5",
    "Elasticsearch_6.7",
    "Elasticsearch_6.8",
    "Elasticsearch_7.1",
    "Elasticsearch_7.4",
    "Elasticsearch_7.7",
    "Elasticsearch_7.8",
    "Elasticsearch_7.9",
    "Elasticsearch_7.10",
    "OpenSearch_1.0",
    "OpenSearch_1.1",
    "OpenSearch_1.2",
    "OpenSearch_1.3",
    "OpenSearch_2.3",
    "OpenSearch_2.5",
    "OpenSearch_2.7",
    "OpenSearch_2.9",
    "OpenSearch_2.11",
    "OpenSearch_2.13",
    "OpenSearch_2.15",
    "OpenSearch_2.17",
    "OpenSearch_2.19",
];

/// The engine version a domain runs when `CreateDomain` names none.
///
/// `EngineVersion` is optional and the service picks a current OpenSearch version
/// when it is omitted. It is one of [`SUPPORTED_ENGINE_VERSIONS`], so a domain
/// created without one runs a version the service publishes.
pub const DEFAULT_ENGINE_VERSION: &str = "OpenSearch_2.11";

/// How far along the upgrade path a version sits: its engine generation (the
/// OpenSearch line succeeds the Elasticsearch one, OpenSearch 1.0 being the fork of
/// Elasticsearch 7.10), then its major and minor numbers compared as NUMBERS, so
/// `7.10` sits after `7.9` rather than before it.
///
/// Comparing two ranks is what "later than" means for an engine version, and it is
/// defined for every value `VersionString`'s pattern allows — in or out of
/// [`SUPPORTED_ENGINE_VERSIONS`]. `None` only for a value matching neither prefix or
/// carrying no `<major>.<minor>`, which that pattern does not allow.
pub fn upgrade_rank(engine_version: &str) -> Option<(u8, u32, u32)> {
    // Which engine a version names is [`engine_type_of`]'s answer, not a second
    // reading of the same prefix: the two cannot drift about what `OpenSearch_` means.
    let generation = match engine_type_of(engine_version)? {
        ENGINE_TYPE_ELASTICSEARCH => 0_u8,
        ENGINE_TYPE_OPENSEARCH => 1_u8,
        // An engine neither constant names has no place in the order rather than
        // sharing the newest line's.
        _ => return None,
    };
    let (major, minor) = engine_version.split_once('_')?.1.split_once('.')?;
    Some((generation, major.parse().ok()?, minor.parse().ok()?))
}

/// The newest published version of the engine line `generation` names, or `None`
/// when [`SUPPORTED_ENGINE_VERSIONS`] publishes none for it.
///
/// Crossing from one engine line to the next starts HERE: the OpenSearch line forked
/// from the LAST Elasticsearch release, so that release is the version a domain has
/// to reach before it can cross.
fn last_published_of(generation: u8) -> Option<(u8, u32, u32)> {
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter_map(|version| upgrade_rank(version))
        .filter(|rank| rank.0 == generation)
        .max()
}

/// The versions a domain running `source_version` can be upgraded to, oldest first:
/// every published version of the SAME engine line strictly LATER than it, plus —
/// only once the domain has reached the last published version of its line — every
/// published version of the line that succeeds it.
///
/// An upgrade only ever goes forwards, so a version is never among its own targets
/// and the newest published version has none. The answer is derived from
/// [`upgrade_rank`]'s comparison rather than from the source being published, so a
/// domain running a version this emulator does not publish still reports the
/// published versions ahead of it on its line.
///
/// This is the whole of the relation: within one line every later version is a
/// target, because no published table of single hops exists to read a narrower
/// within-line rule from. A narrower one belongs HERE when one does.
pub fn compatible_target_versions(source_version: &str) -> Vec<String> {
    let Some(source) = upgrade_rank(source_version) else {
        return Vec::new();
    };
    // A source at or past the last published version of its own line is as far along
    // that line as the service goes, so the next line opens up.
    let crosses_lines = last_published_of(source.0).is_none_or(|last| source >= last);
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|candidate| {
            upgrade_rank(candidate)
                .is_some_and(|rank| rank > source && (rank.0 == source.0 || crosses_lines))
        })
        .map(|candidate| (*candidate).to_string())
        .collect()
}

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

/// How long work this control plane starts takes before the clock owes its outcome:
/// a domain stays `Processing` after the call that started it (`CreateDomain`, or an
/// `UpdateDomainConfig` that changed something) for this long before it owes a
/// settled, `Active` configuration, an outbound cross-cluster connection stays
/// `VALIDATING` for this long before it owes the next status of its lifecycle, and a
/// package stays `COPYING` for this long before it owes the status its copy left it
/// in.
///
/// OpenSearch Service takes many minutes; the emulator chooses a span, and it is
/// deliberately NOT zero so a caller acting straight after the call still observes
/// the transient state and the states that go with it. The due instant is stored on
/// the record and materialised by whichever later call reads it — nothing sleeps.
pub const PROVISIONING: Duration = Duration::from_millis(500);

/// The service software release a domain runs when it is created.
///
/// A service software release is labelled `R<date>`, optionally with a patch suffix
/// (`ServiceUrl`-style opaque values aside, that is the only form the service
/// publishes). A newly created domain runs the current release, so this is the
/// emulator's single statement of what that is — `ListVersions`, which publishes the
/// supported set, is still a stub.
pub const DEFAULT_SERVICE_SOFTWARE_VERSION: &str = "R20250101";

/// How long after a service software deployment lands the domain can still be rolled
/// back onto the release it came off.
///
/// The service publishes no machine-readable value for this window, so the span is
/// the emulator's OWN choice: long enough that a caller who deploys and then rolls
/// back in the same session is inside it, and finite, so a window that has closed is
/// observable. Stored as the instant the window shuts on the record it belongs to —
/// nothing sleeps for it.
pub const ROLLBACK_WINDOW: Duration = Duration::from_secs(72 * 60 * 60);

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

/// `DomainProcessingStatusType`'s `UpdatingServiceSoftware`: a service software
/// deployment is being applied to the domain.
pub const PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE: &str = "UpdatingServiceSoftware";

/// `DeploymentStatus`'s `COMPLETED` value: no service software deployment is in
/// flight on the domain.
pub const DEPLOYMENT_STATUS_COMPLETED: &str = "COMPLETED";

/// `DeploymentStatus`'s `IN_PROGRESS` value: a service software deployment is being
/// applied.
pub const DEPLOYMENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `DeploymentStatus`'s `ELIGIBLE` value: a service software release the domain can
/// be moved onto is available.
pub const DEPLOYMENT_STATUS_ELIGIBLE: &str = "ELIGIBLE";

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

// ---------------------------------------------------------------------------
// Cross-cluster connections
// ---------------------------------------------------------------------------

/// `ConnectionMode`'s `DIRECT` value, documented as the mode "used for cross-cluster
/// search or cross-cluster replication" — which is what `CreateOutboundConnection`
/// creates, so it is the mode a connection is created with when the request names
/// none.
pub const CONNECTION_MODE_DIRECT: &str = "DIRECT";

/// `ConnectionMode`'s `VPC_ENDPOINT` value — the other member of that same two-value
/// enum, "used for remote reindex between Amazon OpenSearch Service VPC domains".
/// The model's own enum is what refuses a mode outside the pair, at the request
/// boundary, before a handler sees it.
pub const CONNECTION_MODE_VPC_ENDPOINT: &str = "VPC_ENDPOINT";

/// `OutboundConnectionStatusCode`'s `VALIDATING`: "The outbound connection request is
/// being validated." The status a connection holds from the instant
/// `CreateOutboundConnection` returns until [`PROVISIONING`] has passed.
pub const CONNECTION_STATUS_VALIDATING: &str = "VALIDATING";

/// `OutboundConnectionStatusCode`'s (and `InboundConnectionStatusCode`'s)
/// `PENDING_ACCEPTANCE`: the request "is validated and is not yet accepted by the
/// remote domain owner" — where a validated connection waits for the destination
/// domain's owner to accept or reject it.
pub const CONNECTION_STATUS_PENDING_ACCEPTANCE: &str = "PENDING_ACCEPTANCE";

/// `OutboundConnectionStatusCode`'s `VALIDATION_FAILED`: "Validation failed for the
/// connection request." Where a request whose referenced domains do not both resolve
/// ends up, instead of waiting for a decision no owner can make.
pub const CONNECTION_STATUS_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// How many cross-cluster connections ONE source domain holds before
/// `CreateOutboundConnection` refuses with the modelled `LimitExceededException`.
///
/// Like [`DATA_SOURCE_QUOTA`] this is the emulator's OWN bound on the store — it
/// models neither Service Quotas nor an account's adjustments — and it is what stops
/// a caller looping with fresh aliases on one source domain from growing the state
/// without end: every record it counts is released by deleting the connection.
///
/// It does NOT on its own bound the whole store, because its partition is the source
/// domain the REQUEST names and a request may name a domain the registry does not
/// hold (such a request is accepted and its validation then fails, which is the
/// behaviour of the service). [`CONNECTION_STORE_QUOTA`] is the bound that does.
pub const CONNECTION_QUOTA: usize = 20;

/// How many cross-cluster connections the WHOLE store holds before
/// `CreateOutboundConnection` refuses with the modelled `LimitExceededException`.
///
/// This is the aggregate bound on the connection store, and the one that holds
/// whatever a caller varies: no request field partitions it. It is set at what the
/// two per-resource quotas allow together — every domain the account can hold, each
/// with a full set of connections — so it never refuses a create that the
/// per-source-domain quota would have allowed on a domain that exists. Because the
/// stored set IS what `DescribeOutboundConnections` answers with, it also bounds the
/// size of that answer.
pub const CONNECTION_STORE_QUOTA: usize = CONNECTION_QUOTA * DOMAIN_QUOTA;

/// A status move the clock owes a record: where the record goes, what there is to
/// say about it, and when it falls due.
///
/// Stored on the record rather than waited for, so nothing sleeps: whichever later
/// call reads the record materialises the move if it is due by then. Shared by every
/// resource of this service that is minted in a transient status — a cross-cluster
/// [`Connection`] being validated, a [`Package`] whose source is being copied — so
/// one mechanism settles them all.
#[derive(Debug, Clone)]
pub struct OwedStatus {
    /// The status value the record moves to, from whichever status enum its own
    /// shape reports.
    pub status: String,
    /// What there is to say about that status beyond the code itself, where the
    /// record's shape carries a message member and there is more to say.
    pub message: Option<String>,
    /// The instant the move falls due.
    pub due: SystemTime,
}

/// One cross-cluster connection between a source (local) domain and a destination
/// (remote) domain, as this emulator persists it.
///
/// A cross-cluster connection is ONE resource with two views: the source domain
/// owner's OUTBOUND connection and the destination owner's INBOUND connection, which
/// share a `ConnectionId` and move through the lifecycle together. This is the single
/// record behind both views — `CreateOutboundConnection` mints it and
/// `DescribeOutboundConnections` reports it — so no two operations can report the
/// same connection differently.
#[derive(Debug, Clone)]
pub struct Connection {
    /// `OutboundConnection$ConnectionId`, which is also the inbound view's. Unique
    /// per connection, so it IS the key the store holds the record under.
    pub connection_id: String,
    /// `OutboundConnection$ConnectionAlias`: the name the create gave the connection.
    pub connection_alias: String,
    /// The source domain this connection is from, as the create's `LocalDomainInfo`
    /// resolved to.
    pub local_domain: AWSDomainInformation,
    /// The destination domain this connection is to, as the create's
    /// `RemoteDomainInfo` resolved to.
    pub remote_domain: AWSDomainInformation,
    /// `OutboundConnection$ConnectionMode`: [`CONNECTION_MODE_DIRECT`] or
    /// [`CONNECTION_MODE_VPC_ENDPOINT`].
    pub connection_mode: String,
    /// `OutboundConnection$ConnectionProperties`, absent when the create carried
    /// none. Stored as the modelled shape rather than a second hand-written mirror
    /// of its members.
    pub connection_properties: Option<ConnectionProperties>,
    /// Where the connection is in its lifecycle: one of
    /// `OutboundConnectionStatusCode`'s values.
    pub status: String,
    /// `OutboundConnectionStatus$Message`: what there is to say about that status
    /// beyond the code — why a validation failed, say. `None` where the code says it
    /// all.
    pub status_message: Option<String>,
    /// The status move the clock owes this connection, if any is outstanding.
    pub owed: Option<OwedStatus>,
}

impl Connection {
    /// Materialise the status move the clock owes this connection, if `now` is past
    /// its due instant: a request that was being validated becomes one waiting for
    /// the remote domain owner's decision, or one whose validation failed.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.owed.as_ref().is_none_or(|owed| now < owed.due) {
            return;
        }
        let Some(owed) = self.owed.take() else {
            return;
        };
        self.status = owed.status;
        self.status_message = owed.message;
    }

    /// This record as `OutboundConnection` — the element shape
    /// `DescribeOutboundConnections` reports, and member-for-member the projection
    /// `CreateOutboundConnection` answers with.
    pub fn to_outbound(&self) -> OutboundConnection {
        OutboundConnection {
            local_domain_info: Some(domain_information(&self.local_domain)),
            remote_domain_info: Some(domain_information(&self.remote_domain)),
            connection_id: Some(self.connection_id.clone()),
            connection_alias: Some(self.connection_alias.clone()),
            connection_status: Some(OutboundConnectionStatus {
                status_code: Some(self.status.clone()),
                // "Verbose information for the outbound connection": carried only
                // where the status has something to say the code does not.
                message: self.status_message.clone(),
            }),
            connection_mode: Some(self.connection_mode.clone()),
            connection_properties: self.connection_properties.clone(),
        }
    }
}

/// A stored domain reference in the container the wire reports it in.
pub fn domain_information(info: &AWSDomainInformation) -> DomainInformationContainer {
    DomainInformationContainer {
        aws_domain_information: Some(info.clone()),
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

/// A service software deployment in flight on a domain: the release it is moving the
/// domain onto, and when the clock owes it completion.
///
/// A deployment is started by a service-software call and completes when the span the
/// clock owes it has passed ([`PROVISIONING`], the same span a configuration change
/// takes) — materialised by whichever later call reads the record, so nothing sleeps.
#[derive(Debug, Clone)]
pub struct SoftwareDeployment {
    /// The release the domain is being moved onto, reported as
    /// `ServiceSoftwareOptions$NewVersion` while the deployment is in flight.
    pub target_version: String,
    /// When the clock owes this deployment completion.
    pub completes_at: SystemTime,
    /// Whether this deployment moves the domain BACK onto an earlier release. A
    /// landed rollback leaves the release it came off available to update to again,
    /// never available to roll back to — so which direction a deployment ran decides
    /// what the domain can do next.
    pub rollback: bool,
}

/// The service software of one domain: the release it runs, the deployment in flight
/// on it, and whether the release it last came off is still within its rollback
/// window.
///
/// This is the ONE record behind `ServiceSoftwareOptions` (reported by every full
/// descriptor of a domain) and `RollbackServiceSoftwareOptions` (reported by
/// `RollbackServiceSoftwareUpdate`), so the two projections can never disagree about
/// which release a domain runs.
#[derive(Debug, Clone)]
pub struct ServiceSoftware {
    /// `ServiceSoftwareOptions$CurrentVersion`: the release the domain is running.
    pub current_version: String,
    /// A release the domain can be moved onto, when one is available and no
    /// deployment is in flight.
    pub available_version: Option<String>,
    /// The deployment in flight, if any.
    pub deployment: Option<SoftwareDeployment>,
    /// The release a rollback would return the domain to: the one it ran before the
    /// deployment that last landed. `None` on a domain no deployment has landed on,
    /// and on one whose last deployment was itself a rollback.
    pub rollback_to: Option<String>,
    /// When the rollback window of the deployment that last landed closes; see
    /// [`ROLLBACK_WINDOW`]. A rollback is available only strictly before it.
    pub rollback_until: Option<SystemTime>,
    /// `ServiceSoftwareOptions$OptionalDeployment`: "True if a service software is
    /// never automatically updated."
    pub optional_deployment: bool,
}

impl ServiceSoftware {
    /// The service software of a domain that has just been created: it runs the
    /// current release, nothing is deploying, and there is no earlier release to roll
    /// back to.
    pub fn initial() -> Self {
        Self {
            current_version: DEFAULT_SERVICE_SOFTWARE_VERSION.to_string(),
            available_version: None,
            deployment: None,
            rollback_to: None,
            rollback_until: None,
            // No automated deployment is scheduled on a domain this emulator holds,
            // so no release of it is ever updated without a caller asking.
            optional_deployment: true,
        }
    }

    /// Whether a deployment is being applied right now.
    pub fn deploying(&self) -> bool {
        self.deployment.is_some()
    }

    /// Materialise the deployment completion the clock owes this record, if `now` is
    /// past its due instant: the release it was moving onto becomes the one running,
    /// and what the domain can do next follows from which direction it ran.
    ///
    /// The landing is dated at the DUE instant rather than at `now`, so the rollback
    /// window a caller reads does not move with the moment it happened to look.
    fn settle(&mut self, now: SystemTime) {
        let Some(deployment) = self.deployment.as_ref() else {
            return;
        };
        if now < deployment.completes_at {
            return;
        }
        let Some(deployment) = self.deployment.take() else {
            return;
        };
        let came_off = ::std::mem::replace(&mut self.current_version, deployment.target_version);
        if deployment.rollback {
            // Rolling back does not open a window to roll forward again: the release
            // the domain came off is simply an update it can take once more.
            self.available_version = Some(came_off);
            self.rollback_to = None;
            self.rollback_until = None;
        } else {
            // The update that just landed is the one a rollback would undo, for as
            // long as its window stays open.
            self.available_version = None;
            self.rollback_to = Some(came_off);
            self.rollback_until = Some(deployment.completes_at + ROLLBACK_WINDOW);
        }
    }

    /// Whether a service software rollback is available on this domain at `now`: a
    /// deployment has landed leaving an earlier release to return to, its window is
    /// still open, and no deployment is in flight to interrupt.
    pub fn rollback_available(&self, now: SystemTime) -> bool {
        !self.deploying()
            && self.rollback_to.is_some()
            && self.rollback_until.is_some_and(|until| now < until)
    }

    /// Start the rollback a call at `now` asked for, or answer `None` when no rollback
    /// is available — which is the whole eligibility test, so a caller cannot start one
    /// the record does not allow.
    ///
    /// The rollback is a deployment like any other: the domain keeps running its
    /// current release until the clock makes the deployment due, and the release it is
    /// returning to is the deployment's target.
    pub fn begin_rollback(&mut self, now: SystemTime) -> Option<RollbackServiceSoftwareOptions> {
        if !self.rollback_available(now) {
            return None;
        }
        let target = self.rollback_to.take()?;
        // The window belonged to the deployment being undone; undoing it consumes it.
        self.rollback_until = None;
        self.deployment = Some(SoftwareDeployment {
            target_version: target.clone(),
            completes_at: now + PROVISIONING,
            rollback: true,
        });
        Some(RollbackServiceSoftwareOptions {
            current_version: Some(self.current_version.clone()),
            new_version: Some(target),
            rollback_available: Some(true),
            description: Some(self.description()),
        })
    }

    /// This record's rollback options when no rollback is available: the release the
    /// domain runs, no release to return to, and `RollbackAvailable` false — the
    /// modelled member for "whether a service software rollback is available for the
    /// domain".
    ///
    /// `Description` is "a description of the rollback status", so it names which part
    /// of the rule the record fails: a deployment already running, a window that has
    /// closed, or no landed deployment to undo at all.
    pub fn rollback_unavailable(&self) -> RollbackServiceSoftwareOptions {
        let description = if self.deploying() {
            "A service software deployment is already in progress on this domain.".to_string()
        } else if self.rollback_to.is_some() {
            "The rollback window of this domain's last service software update has closed."
                .to_string()
        } else {
            "No service software rollback is available for this domain.".to_string()
        };
        RollbackServiceSoftwareOptions {
            current_version: Some(self.current_version.clone()),
            // "The service software version that the domain will roll back to": it
            // will roll back to none.
            new_version: None,
            rollback_available: Some(false),
            description: Some(description),
        }
    }

    /// `ServiceSoftwareOptions$UpdateStatus`, one of `DeploymentStatus`'s values:
    /// `IN_PROGRESS` while a deployment is being applied, `ELIGIBLE` when a release
    /// the domain can take is available, and `COMPLETED` when the domain is settled on
    /// the release it runs.
    pub fn update_status(&self) -> &'static str {
        if self.deploying() {
            DEPLOYMENT_STATUS_IN_PROGRESS
        } else if self.available_version.is_some() {
            DEPLOYMENT_STATUS_ELIGIBLE
        } else {
            DEPLOYMENT_STATUS_COMPLETED
        }
    }

    /// "A description of the service software update status" — derived from the record
    /// rather than stored, so it cannot disagree with the status beside it.
    fn description(&self) -> String {
        match (&self.deployment, &self.available_version) {
            (Some(deployment), _) if deployment.rollback => format!(
                "Service software rollback to {} is in progress.",
                deployment.target_version
            ),
            (Some(deployment), _) => format!(
                "Service software update to {} is in progress.",
                deployment.target_version
            ),
            (None, Some(available)) => {
                format!("Service software update {available} is available.")
            }
            (None, None) => "There is no software update available for this domain.".to_string(),
        }
    }

    /// This record as `ServiceSoftwareOptions` — what every full descriptor of a
    /// domain reports and what the service-software operations answer with.
    pub fn to_options(&self) -> ServiceSoftwareOptions {
        ServiceSoftwareOptions {
            current_version: Some(self.current_version.clone()),
            new_version: self
                .deployment
                .as_ref()
                .map(|deployment| deployment.target_version.clone())
                .or_else(|| self.available_version.clone()),
            update_available: Some(!self.deploying() && self.available_version.is_some()),
            // "True if you're able to cancel your service software version update":
            // only a deployment that has been requested and not yet begun can be
            // cancelled, and a deployment here begins with the call that asks for it,
            // so there is never one waiting.
            cancellable: Some(false),
            update_status: Some(self.update_status().to_string()),
            description: Some(self.description()),
            // "The timestamp ... until which you can manually request a service
            // software update. After this date, we automatically update your service
            // software." No deployment on a domain this emulator holds is automatic,
            // so there is no such date to report.
            automated_update_date: None,
            optional_deployment: Some(self.optional_deployment),
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
    /// The service software this domain runs, and the deployment history the
    /// service-software operations read and write. See [`ServiceSoftware`].
    pub service_software: ServiceSoftware,
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
    /// The packages of this account that are associated with this domain, keyed by
    /// their `PackageID` — a domain holds at most ONE association per package, so
    /// the package's id IS the key. `AssociatePackages` makes one and
    /// `ListPackagesForDomain` reports it, from THIS one record, and deleting the
    /// domain releases every association with it. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads report the same associations in
    /// the same order.
    ///
    /// The set needs no quota of its own: a domain holds at most one association per
    /// package and every association names a package the store holds, so
    /// [`PACKAGE_QUOTA`] bounds it, and [`DOMAIN_QUOTA`] bounds it across domains.
    pub package_associations: BTreeMap<String, PackageAssociation>,
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
        // A service software deployment is owed its completion by the same clock, and
        // on its own span: it is not one of the domain's configuration changes.
        self.service_software.settle(now);
        // An association of a package with this domain is owed its outcome by that
        // same clock, and likewise on its own span.
        for association in self.package_associations.values_mut() {
            association.settle(now);
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
            // A domain applying a service software deployment is processing too, even
            // when no configuration change is in flight.
            processing: Some(self.processing || self.service_software.deploying()),
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
            // The same record `RollbackServiceSoftwareUpdate` answers about, read
            // through its own projection.
            service_software_options: Some(self.service_software.to_options()),
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
            domain_processing_status: Some(if self.service_software.deploying() {
                PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE.to_string()
            } else {
                self.processing_status.clone()
            }),
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

    /// Whether this domain's cluster is UP: its nodes are provisioned, so it has an
    /// elected master, a readable shard allocation and nodes to report.
    ///
    /// A domain is [`PROCESSING_STATUS_CREATING`] from the call that mints it until
    /// the provisioning the clock owes it ([`PROVISIONING`]) falls due, and no node
    /// exists before then. A later configuration change leaves the running cluster
    /// in place, so a [`PROCESSING_STATUS_MODIFYING`] domain is up.
    pub fn cluster_is_up(&self) -> bool {
        self.created && self.processing_status != PROCESSING_STATUS_CREATING
    }
}

/// `AutoTuneState`'s `DISABLED` value: what Auto-Tune's state is when a domain
/// carries Auto-Tune options that name no desired state.
pub const AUTO_TUNE_STATE_DISABLED: &str = "DISABLED";

// ---------------------------------------------------------------------------
// Cluster topology
//
// WHERE a domain's nodes sit: how many Availability Zones its `ClusterConfig`
// places it in, how many of those are held in standby, what each zone is named,
// and how the nodes of one role spread across them. Every operation that reports
// on the provisioned cluster — `DescribeDomainHealth` zone by zone,
// `DescribeDomainNodes` node by node — derives it from HERE, so no two of them
// can describe one domain's topology differently.
// ---------------------------------------------------------------------------

/// The number of Availability Zones a domain sits in when `ZoneAwarenessEnabled`
/// is off: one. `ClusterConfig$ZoneAwarenessEnabled` "Indicates whether multiple
/// Availability Zones are enabled", so a domain without it occupies a single zone.
pub const SINGLE_ZONE: i32 = 1;

/// The numbers of Availability Zones a zone-aware domain can be configured in, from
/// `ZoneAwarenessConfig$AvailabilityZoneCount`: "Valid values are `2` and `3`."
///
/// A stored count outside this range names no number of zones the service supports,
/// so there is no zone count to report and no zone to place a node in. That is also
/// what keeps a per-zone or per-node answer BOUNDED: the count is a request field
/// (`CreateDomain` and `UpdateDomainConfig` persist the caller's `ClusterConfig`
/// verbatim, and `IntegerClass` carries no `@range`), and nothing a caller sends may
/// size an allocation or a response.
pub const ZONE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 2..=3;

/// How many of a multi-AZ domain's zones are held in standby when
/// `MultiAZWithStandbyEnabled` is on: one. From
/// `ClusterConfig$MultiAZWithStandbyEnabled` — "whether a multi-AZ domain is turned
/// on with a standby AZ" — the standby is a single zone.
pub const STANDBY_ZONES: i32 = 1;

/// The letters an Availability Zone name ends in, appended to the Region
/// (`us-east-1` + `a` = `us-east-1a`).
///
/// WHICH of a Region's zones an account's domain lands in is an account-specific
/// mapping AWS does not publish, so the emulator names a domain's zones in order
/// from the Region's first zone letter. `AvailabilityZone` is capped at 15
/// characters, which `<region><letter>` stays inside for every published Region
/// name.
const ZONE_LETTERS: &[u8] = b"abcdefgh";

/// The name of the `index`-th Availability Zone of `region`; `None` once the zone
/// index runs past [`ZONE_LETTERS`], since the zone then has no name to report.
pub fn zone_name(region: &str, index: usize) -> Option<String> {
    let letter = char::from(*ZONE_LETTERS.get(index)?);
    Some(format!("{region}{letter}"))
}

/// The number of Availability Zones a cluster configuration places its domain in:
/// the zone-awareness count when multiple zones are turned on
/// (`ZoneAwarenessConfig$AvailabilityZoneCount` is "the number of zones that you
/// want the domain to use"), and [`SINGLE_ZONE`] when they are not.
///
/// `None` when zone awareness is on and the record carries no count for it, or one
/// outside [`ZONE_COUNT_RANGE`] — the configuration then names no supported number
/// of zones, so there is none to report and nothing sized by it to build.
pub fn configured_zone_count(cluster: &ClusterConfig) -> Option<i32> {
    if cluster.zone_awareness_enabled == Some(true) {
        cluster
            .zone_awareness_config
            .as_ref()
            .and_then(|config| config.availability_zone_count)
            .filter(|zones| ZONE_COUNT_RANGE.contains(zones))
    } else {
        Some(SINGLE_ZONE)
    }
}

/// How many of the domain's Availability Zones are held in standby:
/// [`STANDBY_ZONES`] for a cluster configured with a standby zone, none otherwise.
pub fn standby_zone_count(cluster: &ClusterConfig) -> i32 {
    if cluster.multi_az_with_standby_enabled == Some(true) {
        STANDBY_ZONES
    } else {
        0
    }
}

/// How a role's `nodes` sit across the domain's `zones` Availability Zones: zone
/// awareness spreads them evenly, so each zone holds `nodes / zones` of them and
/// the first `nodes % zones` zones hold one more.
///
/// `zones` is the already-bounded count [`configured_zone_count`] answers — at most
/// the top of [`ZONE_COUNT_RANGE`] — so the vector this builds is bounded too, and
/// is capped again here rather than trusting its caller.
///
/// `None` for a node count the record does not carry: the nodes per zone are then
/// unknown too.
pub fn nodes_per_zone(nodes: Option<i32>, zones: i32) -> Vec<Option<i32>> {
    let zones = zones.clamp(0, *ZONE_COUNT_RANGE.end());
    let zone_count = usize::try_from(zones).unwrap_or(0);
    let Some(nodes) = nodes.filter(|nodes| *nodes >= 0).filter(|_| zone_count > 0) else {
        return vec![None; zone_count];
    };
    (0..zones)
        .map(|zone| Some(nodes / zones + i32::from(zone < nodes % zones)))
        .collect()
}

// ---------------------------------------------------------------------------
// The Reserved Instance offering catalog
// ---------------------------------------------------------------------------

/// `ReservedInstancePaymentOption`'s `ALL_UPFRONT` value: the whole term is paid as
/// one upfront charge, so the reservation carries no recurring charge at all.
pub const PAYMENT_OPTION_ALL_UPFRONT: &str = "ALL_UPFRONT";

/// `ReservedInstancePaymentOption`'s `PARTIAL_UPFRONT` value: part of the term is
/// paid upfront and the rest as a recurring charge.
pub const PAYMENT_OPTION_PARTIAL_UPFRONT: &str = "PARTIAL_UPFRONT";

/// `ReservedInstancePaymentOption`'s `NO_UPFRONT` value: nothing is paid upfront and
/// the whole term is billed as a recurring charge.
pub const PAYMENT_OPTION_NO_UPFRONT: &str = "NO_UPFRONT";

/// `ReservedInstanceOffering$Duration` for a one-year term, in seconds — the unit
/// that member is documented in ("the duration, in seconds, for which the offering
/// will reserve the OpenSearch instance").
pub const TERM_ONE_YEAR: i32 = 365 * 24 * 60 * 60;

/// `ReservedInstanceOffering$Duration` for a three-year term, in seconds.
pub const TERM_THREE_YEARS: i32 = 3 * TERM_ONE_YEAR;

/// The terms the offering catalog publishes, shortest first: OpenSearch Service
/// reserves an instance for one year or for three.
pub const RESERVED_INSTANCE_TERMS: [i32; 2] = [TERM_ONE_YEAR, TERM_THREE_YEARS];

/// `ReservedInstanceOffering$CurrencyCode`: every price in the catalog is quoted in
/// US dollars, the currency AWS publishes its price list in.
pub const RESERVED_INSTANCE_CURRENCY_CODE: &str = "USD";

/// `RecurringCharge$RecurringChargeFrequency` for a reservation's recurring charge:
/// the charge is billed per hour of the term.
pub const RECURRING_CHARGE_HOURLY: &str = "Hourly";

/// How many seconds make an hour, for turning a term's `Duration` into the hours a
/// recurring charge is billed over.
const SECONDS_PER_HOUR: f64 = 3600.0;

/// AWS's published instance-size normalization factors, keyed by the size token of an
/// `OpenSearchPartitionInstanceType` (which is `<family>.<size>.search`). They are
/// the factors AWS publishes for Reserved Instance size flexibility — one `large` is
/// four units, one `xlarge` eight, and so on — and they are what makes a bigger size
/// of one family cost proportionally more in [`on_demand_hourly_price`].
const SIZE_NORMALIZATION_FACTORS: [(&str, f64); 15] = [
    ("nano", 0.25),
    ("micro", 0.5),
    ("small", 1.0),
    ("medium", 2.0),
    ("large", 4.0),
    ("xlarge", 8.0),
    ("2xlarge", 16.0),
    ("4xlarge", 32.0),
    ("8xlarge", 64.0),
    ("9xlarge", 72.0),
    ("10xlarge", 80.0),
    ("12xlarge", 96.0),
    ("16xlarge", 128.0),
    ("18xlarge", 144.0),
    ("24xlarge", 192.0),
];

/// The hourly on-demand rate, in US dollars, of ONE normalization unit of an instance
/// class, keyed by the first letter of the instance family (`c5`, `m6g`, `or1`,
/// `ultrawarm1`, …).
///
/// This is the emulator's OWN price list: AWS's price list is not in the Smithy model
/// and is not published as part of the API, so there is nothing to read it from. What
/// the catalog therefore guarantees is the RELATION between its prices, not an AWS
/// price: a compute-optimized unit costs less than a memory-optimized one, a burstable
/// one less than either, and the size factor scales each. Every price the catalog
/// reports is derived from this table, so the same offering costs the same on every
/// call and in every run.
const CLASS_HOURLY_RATE_PER_UNIT: [(char, f64); 8] = [
    // Burstable (`t2`, `t3`, `t4g`).
    ('t', 0.018),
    // Compute optimized (`c4`, `c5`, `c6g`).
    ('c', 0.034),
    // General purpose (`m3`, `m4`, `m5`, `m6g`).
    ('m', 0.036),
    // OpenSearch optimized (`or1`).
    ('o', 0.040),
    // Memory optimized (`r3`, `r4`, `r5`, `r6g`, `r6gd`).
    ('r', 0.047),
    // Dense storage (`d2`).
    ('d', 0.052),
    // Storage optimized (`i2`, `i3`).
    ('i', 0.055),
    // UltraWarm (`ultrawarm1`).
    ('u', 0.060),
];

/// The fraction of the on-demand rate a ONE-year reservation costs, before the
/// payment-option discount. The emulator's own figure, like
/// [`CLASS_HOURLY_RATE_PER_UNIT`]; what it fixes is that reserving is cheaper than
/// paying on demand.
const ONE_YEAR_RATE: f64 = 0.78;

/// The same fraction for a THREE-year reservation: a longer commitment costs less per
/// hour than a shorter one, so this is below [`ONE_YEAR_RATE`].
const THREE_YEAR_RATE: f64 = 0.52;

/// The further discount for paying the whole term upfront — the largest of the three,
/// so `ALL_UPFRONT` is the cheapest payment option of a term.
const ALL_UPFRONT_DISCOUNT: f64 = 0.92;

/// The further discount for paying part of the term upfront: between
/// [`ALL_UPFRONT_DISCOUNT`] and [`NO_UPFRONT_DISCOUNT`].
const PARTIAL_UPFRONT_DISCOUNT: f64 = 0.96;

/// Paying nothing upfront earns no further discount, so this payment option is the
/// most expensive of a term.
const NO_UPFRONT_DISCOUNT: f64 = 1.0;

/// The hourly on-demand rate of one instance type: its class's rate per normalization
/// unit times its size's factor.
///
/// `None` for a value that is not `<family>.<size>.search` with a known size token or
/// instance class — which no value of `OpenSearchPartitionInstanceType` is, so the
/// catalog spans the whole enum.
fn on_demand_hourly_price(instance_type: &str) -> Option<f64> {
    let mut parts = instance_type.split('.');
    let family = parts.next()?;
    let size = parts.next()?;
    let factor = SIZE_NORMALIZATION_FACTORS
        .iter()
        .find(|(token, _)| *token == size)
        .map(|(_, factor)| *factor)?;
    let class = family.chars().next()?;
    let rate = CLASS_HOURLY_RATE_PER_UNIT
        .iter()
        .find(|(letter, _)| *letter == class)
        .map(|(_, rate)| *rate)?;
    Some(rate * factor)
}

/// The fraction of the on-demand rate a reservation of this term and payment option
/// costs: the term's rate times the payment option's discount.
///
/// `None` for a payment option outside `ReservedInstancePaymentOption`, so a value
/// added to that enum is left out of the catalog rather than silently priced as one
/// of the three it does know.
fn reserved_rate(duration: i32, payment_option: &str) -> Option<f64> {
    let term = if duration >= TERM_THREE_YEARS {
        THREE_YEAR_RATE
    } else {
        ONE_YEAR_RATE
    };
    let upfront = match payment_option {
        PAYMENT_OPTION_ALL_UPFRONT => ALL_UPFRONT_DISCOUNT,
        PAYMENT_OPTION_PARTIAL_UPFRONT => PARTIAL_UPFRONT_DISCOUNT,
        PAYMENT_OPTION_NO_UPFRONT => NO_UPFRONT_DISCOUNT,
        _ => return None,
    };
    Some(term * upfront)
}

/// `value` rounded to whole cents, the precision an upfront charge is quoted at.
fn cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `value` rounded to the precision AWS quotes an hourly rate at — thousandths of a
/// cent, so a small instance's hourly charge does not round away to zero.
fn hourly(value: f64) -> f64 {
    (value * 100_000.0).round() / 100_000.0
}

/// The `ReservedInstanceOfferingId` of one catalog entry.
///
/// DERIVED from what the entry IS (its instance type, term and payment option) as a
/// name-based v5 UUID, so it satisfies `GUID`'s
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`, two
/// entries can never share an id, and the id a caller read in one call is the id the
/// next call reports — which a freshly generated id would not be.
fn offering_id(instance_type: &str, duration: i32, payment_option: &str) -> String {
    ::uuid::Uuid::new_v5(
        &::uuid::Uuid::NAMESPACE_OID,
        format!("opensearch/reservedInstanceOffering/{instance_type}/{duration}/{payment_option}")
            .as_bytes(),
    )
    .to_string()
}

/// One catalog entry: what one instance type costs to reserve for one term under one
/// payment option.
///
/// The three price members hold together the way a reservation is billed: the upfront
/// charge is `FixedPrice`, the rest of the term is a recurring hourly charge, and
/// `UsagePrice` — the rate charged for USING the reserved capacity — is zero, because
/// the reservation has already paid for the hours. So `ALL_UPFRONT` carries a fixed
/// price and no recurring charge, `NO_UPFRONT` a recurring charge and no fixed price,
/// and `PARTIAL_UPFRONT` half the term each way.
fn reserved_instance_offering(
    instance_type: &str,
    duration: i32,
    payment_option: &str,
) -> Option<ReservedInstanceOffering> {
    let on_demand = on_demand_hourly_price(instance_type)?;
    let effective_hourly = on_demand * reserved_rate(duration, payment_option)?;
    let term_hours = f64::from(duration) / SECONDS_PER_HOUR;
    let (fixed_price, recurring_hourly) = match payment_option {
        PAYMENT_OPTION_ALL_UPFRONT => (effective_hourly * term_hours, 0.0),
        PAYMENT_OPTION_PARTIAL_UPFRONT => {
            (effective_hourly * term_hours / 2.0, effective_hourly / 2.0)
        }
        PAYMENT_OPTION_NO_UPFRONT => (0.0, effective_hourly),
        _ => return None,
    };
    let recurring_charges = if recurring_hourly > 0.0 {
        vec![RecurringCharge {
            recurring_charge_amount: Some(hourly(recurring_hourly)),
            recurring_charge_frequency: Some(RECURRING_CHARGE_HOURLY.to_string()),
        }]
    } else {
        // An offering with nothing left to bill reports an EMPTY list rather than no
        // member: the member is "the recurring charge to your account", and for this
        // offering there is none.
        Vec::new()
    };
    Some(ReservedInstanceOffering {
        reserved_instance_offering_id: Some(offering_id(instance_type, duration, payment_option)),
        instance_type: Some(instance_type.to_string()),
        duration,
        fixed_price: Some(cents(fixed_price)),
        usage_price: Some(0.0),
        currency_code: Some(RESERVED_INSTANCE_CURRENCY_CODE.to_string()),
        payment_option: Some(payment_option.to_string()),
        recurring_charges: Some(recurring_charges),
    })
}

/// The whole offering catalog, keyed by `ReservedInstanceOfferingId`.
///
/// Reserved Instance offerings are published BY the service for a Region, not created
/// by a caller, so the catalog is seeded with the state rather than grown by an
/// operation: it is what `DescribeReservedInstanceOfferings` reports and what a
/// purchase resolves an offering id against. It spans one entry per instance type the
/// model's `OpenSearchPartitionInstanceType` enum declares — the only published record
/// of which instance types exist — times each term in [`RESERVED_INSTANCE_TERMS`] and
/// each value of `ReservedInstancePaymentOption`, so it is bounded and complete.
fn seed_reserved_instance_offerings() -> BTreeMap<String, ReservedInstanceOffering> {
    let mut catalog = BTreeMap::new();
    for instance_type in OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES {
        for duration in RESERVED_INSTANCE_TERMS {
            for payment_option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
                let Some(offering) =
                    reserved_instance_offering(instance_type, duration, payment_option)
                else {
                    continue;
                };
                // The id the record CARRIES is the key it is stored under, so the id
                // has one producer per entry and no stored entry can report an id
                // other than the one a caller resolves it by — which is also the id
                // paging resumes from.
                let Some(id) = offering.reserved_instance_offering_id.clone() else {
                    continue;
                };
                catalog.insert(id, offering);
            }
        }
    }
    catalog
}

// ---------------------------------------------------------------------------
// The instance-type catalog
//
// Which instance types an engine version offers, and which features each of them
// supports, is content published BY the service: no caller creates an instance
// type, and the answer depends only on the version asked about. So the catalog is
// DERIVED from the model's published instance-type enum and the version, rather
// than stored per account — which is what makes two reads of one version report the
// same types, in the same order, with the same features.
// ---------------------------------------------------------------------------

/// `InstanceTypeDetails$InstanceRole` for an instance that holds the domain's data,
/// from that member's own documentation: "whether the instance acts as a data node, a
/// dedicated master node, or an UltraWarm node".
pub const INSTANCE_ROLE_DATA: &str = "data";

/// `InstanceTypeDetails$InstanceRole` for an instance that can serve as the cluster's
/// dedicated master node.
pub const INSTANCE_ROLE_MASTER: &str = "master";

/// `InstanceTypeDetails$InstanceRole` for an instance that serves the UltraWarm
/// storage tier.
pub const INSTANCE_ROLE_ULTRAWARM: &str = "ultrawarm";

/// An `EngineVersion` read apart: the engine it names and its `X.Y` release numbers,
/// which is what every version gate in this section is expressed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineVersion {
    /// The `EngineType` the version names: [`ENGINE_TYPE_OPENSEARCH`] or
    /// [`ENGINE_TYPE_ELASTICSEARCH`], as [`engine_type_of`] derives it.
    pub engine: &'static str,
    /// The major release number (the `X` of `X.Y`).
    pub major: u32,
    /// The minor release number (the `Y` of `X.Y`).
    pub minor: u32,
}

impl EngineVersion {
    /// Whether this version runs the OpenSearch engine rather than the legacy
    /// Elasticsearch one.
    pub fn is_opensearch(self) -> bool {
        self.engine == ENGINE_TYPE_OPENSEARCH
    }

    /// Whether this version is at or past an `X.Y` release OF THE ENGINE IT NAMES.
    pub fn at_least(self, (major, minor): (u32, u32)) -> bool {
        (self.major, self.minor) >= (major, minor)
    }

    /// Whether a capability introduced in an Elasticsearch release is available here.
    ///
    /// OpenSearch 1.0 forked from Elasticsearch 7.10, so an OpenSearch version
    /// carries every capability an Elasticsearch release up to that fork introduced —
    /// which each gate in this section is — and an Elasticsearch version carries it
    /// from the release it arrived in.
    pub fn since_elasticsearch(self, release: (u32, u32)) -> bool {
        self.is_opensearch() || self.at_least(release)
    }

    /// Whether this version is an OpenSearch release at or past `X.Y` — the gate for
    /// a capability that never existed on Elasticsearch.
    pub fn opensearch_at_least(self, release: (u32, u32)) -> bool {
        self.is_opensearch() && self.at_least(release)
    }
}

/// An `EngineVersion` read apart, or `None` for a value that is not
/// `Elasticsearch_X.Y` or `OpenSearch_X.Y`.
///
/// `VersionString`'s pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$` allows
/// nothing else, but the request boundary enforces only that shape's LENGTH bound, so
/// an operation that reads a version apart checks the form itself rather than
/// treating an unreadable value as a version.
pub fn parse_engine_version(engine_version: &str) -> Option<EngineVersion> {
    let engine = engine_type_of(engine_version)?;
    let (_, release) = engine_version.split_once('_')?;
    let (major, minor) = release.split_once('.')?;
    Some(EngineVersion {
        engine,
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
    })
}

/// The Elasticsearch release both encryption features `InstanceTypeDetails$Encryption\
/// Enabled` covers are available from. That member is "whether encryption at rest AND
/// node-to-node encryption are supported for the instance type", so the gate is the
/// LATER of the two: node-to-node encryption arrived in Elasticsearch 6.0, encryption
/// at rest before it.
const ENCRYPTION_SINCE_ELASTICSEARCH: (u32, u32) = (6, 0);

/// The Elasticsearch release Amazon Cognito authentication for Dashboards is
/// available from (`InstanceTypeDetails$CognitoEnabled`).
const COGNITO_SINCE_ELASTICSEARCH: (u32, u32) = (5, 1);

/// The Elasticsearch release publishing application logs to CloudWatch Logs is
/// available from (`InstanceTypeDetails$AppLogsEnabled`), which is also the release
/// the `LogPublishingOptions` container appeared with.
const APP_LOGS_SINCE_ELASTICSEARCH: (u32, u32) = (5, 1);

/// The Elasticsearch release fine-grained access control is available from
/// (`InstanceTypeDetails$AdvancedSecurityEnabled`).
const ADVANCED_SECURITY_SINCE_ELASTICSEARCH: (u32, u32) = (6, 7);

/// The Elasticsearch release UltraWarm storage is available from
/// (`InstanceTypeDetails$WarmEnabled`), which is also the release the UltraWarm
/// instance family was introduced for.
const ULTRAWARM_SINCE_ELASTICSEARCH: (u32, u32) = (6, 8);

/// The Elasticsearch release the Graviton instance families are offered from.
const GRAVITON_SINCE_ELASTICSEARCH: (u32, u32) = (7, 9);

/// The OpenSearch release the OpenSearch-optimized instance family is offered from.
/// It is an OpenSearch-only family, so this gate is not an Elasticsearch one.
const OPENSEARCH_OPTIMIZED_SINCE: (u32, u32) = (2, 11);

/// The instance families neither encryption at rest nor node-to-node encryption is
/// available on, whatever the engine version
/// (`InstanceTypeDetails$EncryptionEnabled`). Fine-grained access control requires
/// both, so it is unavailable on them too.
const UNENCRYPTED_FAMILIES: [&str; 2] = ["m3", "t2"];

/// The burstable instance families. UltraWarm cannot be attached to a cluster whose
/// hot tier runs on one of them, so `InstanceTypeDetails$WarmEnabled` is off for
/// them.
const BURSTABLE_FAMILIES: [&str; 3] = ["t2", "t3", "t4g"];

/// The instance family that serves the UltraWarm tier: its instances act as UltraWarm
/// nodes rather than as data or master nodes, which is the `InstanceRole` they carry.
const ULTRAWARM_FAMILY: &str = "ultrawarm1";

/// The Graviton instance families, offered from
/// [`GRAVITON_SINCE_ELASTICSEARCH`] onwards.
const GRAVITON_FAMILIES: [&str; 5] = ["c6g", "m6g", "r6g", "r6gd", "t4g"];

/// The OpenSearch-optimized instance families: offered from
/// [`OPENSEARCH_OPTIMIZED_SINCE`] onwards, and usable as data nodes only, so an
/// instance of one is never master-eligible.
const OPENSEARCH_OPTIMIZED_FAMILIES: [&str; 1] = ["or1"];

/// The family token of an `OpenSearchPartitionInstanceType`, which is
/// `<family>.<size>.search`; `None` for a value carrying no family at all.
pub fn instance_family(instance_type: &str) -> Option<&str> {
    instance_type
        .split('.')
        .next()
        .filter(|family| !family.is_empty())
}

/// Whether `version` offers `instance_type` at all.
///
/// An instance family introduced for a later engine release is not offered on an
/// earlier one; every other family the model publishes is offered on every version.
pub fn offers_instance_type(version: EngineVersion, instance_type: &str) -> bool {
    let Some(family) = instance_family(instance_type) else {
        return false;
    };
    if family == ULTRAWARM_FAMILY {
        return version.since_elasticsearch(ULTRAWARM_SINCE_ELASTICSEARCH);
    }
    if GRAVITON_FAMILIES.contains(&family) {
        return version.since_elasticsearch(GRAVITON_SINCE_ELASTICSEARCH);
    }
    if OPENSEARCH_OPTIMIZED_FAMILIES.contains(&family) {
        return version.opensearch_at_least(OPENSEARCH_OPTIMIZED_SINCE);
    }
    true
}

/// Every instance type `version` offers, in the order the model publishes them.
///
/// `OpenSearchPartitionInstanceType` is the model's record of which instance types
/// exist, so it is what the catalog spans — the same enum the Reserved Instance
/// offering catalog above is built over. The order is that enum's, which is stable,
/// so a paged read of the catalog resumes where the previous page stopped.
pub fn offered_instance_types(version: EngineVersion) -> Vec<&'static str> {
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES
        .iter()
        .copied()
        .filter(|instance_type| offers_instance_type(version, instance_type))
        .collect()
}

/// What `instance_type` supports on `version`, as `InstanceTypeDetails`.
///
/// Each feature member is a conjunction of two things: the engine release the feature
/// is available from, and whether the instance family can serve it at all. The roles
/// come from the family: the UltraWarm family serves the warm tier, an
/// OpenSearch-optimized instance is a data node only, and every other family is both
/// a data node and master-eligible.
///
/// `availability_zones` is reported only when the caller asked for it, so a request
/// that did not carries no `AvailabilityZones` member rather than an empty list.
pub fn instance_type_details(
    version: EngineVersion,
    instance_type: &str,
    availability_zones: Option<Vec<String>>,
) -> InstanceTypeDetails {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    let encryption = !UNENCRYPTED_FAMILIES.contains(&family)
        && version.since_elasticsearch(ENCRYPTION_SINCE_ELASTICSEARCH);
    let instance_role = if family == ULTRAWARM_FAMILY {
        vec![INSTANCE_ROLE_ULTRAWARM.to_string()]
    } else if OPENSEARCH_OPTIMIZED_FAMILIES.contains(&family) {
        vec![INSTANCE_ROLE_DATA.to_string()]
    } else {
        vec![
            INSTANCE_ROLE_DATA.to_string(),
            INSTANCE_ROLE_MASTER.to_string(),
        ]
    };
    InstanceTypeDetails {
        instance_type: Some(instance_type.to_string()),
        encryption_enabled: Some(encryption),
        cognito_enabled: Some(version.since_elasticsearch(COGNITO_SINCE_ELASTICSEARCH)),
        app_logs_enabled: Some(version.since_elasticsearch(APP_LOGS_SINCE_ELASTICSEARCH)),
        // Fine-grained access control requires both encryption features, so it is
        // available only where they are.
        advanced_security_enabled: Some(
            encryption && version.since_elasticsearch(ADVANCED_SECURITY_SINCE_ELASTICSEARCH),
        ),
        warm_enabled: Some(
            version.since_elasticsearch(ULTRAWARM_SINCE_ELASTICSEARCH)
                && !BURSTABLE_FAMILIES.contains(&family),
        ),
        instance_role: Some(instance_role),
        availability_zones,
    }
}

/// How many Availability Zones of a Region the instance-type catalog reports an
/// instance type as offered in: the most zones a domain can be spread across, which
/// is the top of [`ZONE_COUNT_RANGE`] (`ZoneAwarenessConfig$AvailabilityZoneCount`:
/// "Valid values are `2` and `3`").
pub const CATALOG_AVAILABILITY_ZONES: usize = 3;

/// The Availability Zones of `region` an instance type is offered in: the Region's
/// first [`CATALOG_AVAILABILITY_ZONES`] zones, named by [`zone_name`] so a zone has
/// one name across this service.
pub fn region_availability_zones(region: &str) -> Vec<String> {
    (0..CATALOG_AVAILABILITY_ZONES)
        .filter_map(|index| zone_name(region, index))
        .collect()
}

/// The `ValidationException` payload for a member that must carry an engine version
/// and does not. It names the member and the value that could not be read as one, so
/// the caller can tell which input it was.
pub fn malformed_engine_version(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must be Elasticsearch_X.Y or OpenSearch_X.Y: {value}"
        )),
    }
}

/// The `ValidationException` payload for a member that must name an instance type the
/// service publishes and does not.
///
/// `InstanceTypeString`'s pattern `^.*\..*\.search$` admits any value of that shape,
/// so a value outside `OpenSearchPartitionInstanceType` — the model's record of which
/// instance types exist — names no instance type at all, which is a malformed request
/// rather than a listing with nothing in it.
pub fn unknown_instance_type(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} is not a known instance type: {value}")),
    }
}

// ---------------------------------------------------------------------------
// Packages
//
// A package is an ACCOUNT resource of this control plane rather than a
// sub-resource of a domain: `CreatePackage` mints one and `DescribePackages`
// reports it, before any domain is associated with it. [`Package`] below is the
// ONE copy every package operation reads and writes, and [`Package::to_details`]
// the single `PackageDetails` projection they report it through, so no two of them
// can describe the same package differently.
// ---------------------------------------------------------------------------

/// `PackageStatus`'s `COPYING`: the package's content is being copied out of the
/// Amazon S3 location the create named.
pub const PACKAGE_STATUS_COPYING: &str = "COPYING";

/// `PackageStatus`'s `AVAILABLE`: the package's content is in place and the package
/// can be associated with a domain.
pub const PACKAGE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `PackageType`'s `ZIP-PLUGIN` value: the only type an engine version may be given
/// for, because `CreatePackageRequest$EngineVersion` "can only be specified for
/// package type ZIP-PLUGIN".
pub const PACKAGE_TYPE_ZIP_PLUGIN: &str = "ZIP-PLUGIN";

/// The `PackageVersion` a package's first available content carries.
///
/// `PackageVersion` is a plain string in the model and the version history is a
/// SEQUENCE, so a package's versions are numbered from one and each update of its
/// content adds the next number.
pub const FIRST_PACKAGE_VERSION: u32 = 1;

/// How many packages this account holds before `CreatePackage` refuses with the
/// modelled `LimitExceededException`.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it models
/// neither Service Quotas nor an account's adjustments — and it is what stops a
/// caller looping with fresh names from growing the state without end: every record
/// it counts is released by deleting the package. Because the stored set IS what
/// `DescribePackages` answers with, it also bounds the size of that answer.
pub const PACKAGE_QUOTA: usize = 25;

/// One custom package of this account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Package {
    /// `PackageDetails$PackageID`: unique per package, so it IS the key the store
    /// holds the record under.
    pub package_id: String,
    /// `PackageDetails$PackageName`: the name the create gave the package, unique
    /// across the account's packages.
    pub package_name: String,
    /// `PackageDetails$PackageType`: one of `PackageType`'s values.
    pub package_type: String,
    /// `PackageDetails$PackageDescription`, absent when the create carried none.
    pub description: Option<String>,
    /// The Amazon S3 location the package's content was imported from. Stored as the
    /// modelled shape rather than a second hand-written mirror of its members; no
    /// read path reports it, because `PackageDetails` declares no member for it.
    pub source: PackageSource,
    /// Where the package is in its lifecycle: one of `PackageStatus`'s values.
    pub status: String,
    /// The status move the clock owes this package, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// Which numbered version of its content the package currently has, counting
    /// from [`FIRST_PACKAGE_VERSION`].
    pub version: u32,
    /// `PackageDetails$EngineVersion`, absent for a package that is not tied to one.
    pub engine_version: Option<String>,
    /// `PackageDetails$AvailablePackageConfiguration`, absent when the create carried
    /// no configuration.
    pub configuration: Option<PackageConfiguration>,
    /// `PackageDetails$PackageVendingOptions`, absent when the create carried none.
    pub vending_options: Option<PackageVendingOptions>,
    /// `PackageDetails$PackageEncryptionOptions`, absent when the create carried
    /// none.
    pub encryption_options: Option<PackageEncryptionOptions>,
    /// The accounts the package is vended to, as `PackageUser` values. Empty until a
    /// scope update allow-lists one.
    pub allow_listed_users: Vec<String>,
    /// `PackageDetails$PackageOwner`: the account the package belongs to.
    pub owner: String,
    /// `PackageDetails$CreatedAt`.
    pub created_at: SystemTime,
    /// `PackageDetails$LastUpdatedAt`: when the record last changed.
    pub updated_at: SystemTime,
}

impl Package {
    /// Materialise the status move the clock owes this package, if `now` is past its
    /// due instant: a package whose content was being copied becomes one the copy has
    /// left ready to use.
    ///
    /// This is the only write a reporting operation makes, and it writes only what was
    /// already owed at the same instant had nobody called — which is also why the
    /// record is stamped with the instant the move fell DUE rather than the instant of
    /// the call that happened to observe it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
        self.updated_at = owed.due;
    }

    /// This record as `PackageDetails` — the shape `CreatePackage` answers with and
    /// the element shape `DescribePackages` reports, so a create's answer and a later
    /// read of the package cannot disagree.
    pub fn to_details(&self) -> PackageDetails {
        PackageDetails {
            package_id: Some(self.package_id.clone()),
            package_name: Some(self.package_name.clone()),
            package_type: Some(self.package_type.clone()),
            package_description: self.description.clone(),
            package_status: Some(self.status.clone()),
            created_at: Some(datetime(self.created_at)),
            last_updated_at: Some(datetime(self.updated_at)),
            // "The package version", meaning the version a domain association would
            // get: there is one only once the content is in place.
            available_package_version: (self.status == PACKAGE_STATUS_AVAILABLE)
                .then(|| self.version.to_string()),
            // "Additional information if the package is in an error state" — this
            // package is not, so there is nothing to report.
            error_details: None,
            engine_version: self.engine_version.clone(),
            // The plugin's name, class and uncompressed size are read out of the
            // descriptor inside the plugin's own archive, which this emulator does not
            // fetch, so they stay absent rather than being invented.
            available_plugin_properties: None,
            available_package_configuration: self.configuration.clone(),
            // "Lists the accounts that the package was shared with": reported only
            // once the package has been shared with one.
            allow_listed_user_list: (!self.allow_listed_users.is_empty())
                .then(|| self.allow_listed_users.clone()),
            package_owner: Some(self.owner.clone()),
            package_vending_options: self.vending_options.clone(),
            package_encryption_options: self.encryption_options.clone(),
        }
    }
}

/// The `PackageID` a create mints.
///
/// `PackageID` is pattern `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`: the `[FG][0-9]+` form
/// addresses a package the service itself publishes, so a package a CALLER creates
/// takes the `pkg-` form, whose body is lowercase hexadecimal. The body is a v4 UUID
/// in its unhyphenated rendering — the same generator as every other id this service
/// mints ([`new_reserved_instance_id`]) in the only rendering this pattern admits —
/// so no two packages share an id.
pub fn new_package_id() -> String {
    format!("pkg-{}", ::uuid::Uuid::new_v4().simple())
}

/// The `ResourceAlreadyExistsException` payload for a create naming a package the
/// account already holds — the mirror of [`connection_alias_exists`], so one refusal
/// reads like the other.
pub fn package_name_exists(package_name: &str) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!("Package already exists: {package_name}")),
    }
}

// ---------------------------------------------------------------------------
// Package associations
//
// An association is the LINK between one domain and one package of this
// account: `AssociatePackages` makes one and `ListPackagesForDomain` reports
// it. It is held on the [`Domain`] it links, under the `PackageID` of the
// package it links to, so no map beside the domain can disagree about which
// packages that domain has, and releasing the domain releases its associations.
//
// The association carries only what belongs to the LINK — its lifecycle status,
// the package version it put on the domain, its prerequisites and its
// configuration. The package's own name and type are read from the one stored
// [`Package`] record through [`PackageAssociation::to_details`], so a package
// operation and an association one cannot report the same package differently.
// ---------------------------------------------------------------------------

/// `DomainPackageStatus`'s `ASSOCIATING`: the association has been asked for and
/// the package is being put in place on the domain.
pub const DOMAIN_PACKAGE_STATUS_ASSOCIATING: &str = "ASSOCIATING";

/// `DomainPackageStatus`'s `ACTIVE`: the package is in place on the domain, at the
/// version the association carries.
pub const DOMAIN_PACKAGE_STATUS_ACTIVE: &str = "ACTIVE";

/// `DomainPackageStatus`'s `DISSOCIATING`: the package is being removed from the
/// domain.
pub const DOMAIN_PACKAGE_STATUS_DISSOCIATING: &str = "DISSOCIATING";

/// `PackageType`'s `TXT-DICTIONARY` value: the dictionary content a domain's index
/// settings reference BY PATH, which is what gives an association of it a
/// `ReferencePath`.
pub const PACKAGE_TYPE_TXT_DICTIONARY: &str = "TXT-DICTIONARY";

/// The directory a domain's index settings reference an associated dictionary
/// package under, from the form the service documents for that reference
/// (`analyzers/<PackageID>`).
const ANALYZERS_DIRECTORY: &str = "analyzers";

/// One link between a domain and a package of this account, as this emulator
/// persists it.
#[derive(Debug, Clone)]
pub struct PackageAssociation {
    /// The `PackageID` of the package this association links to, which is also the
    /// key the domain holds it under.
    pub package_id: String,
    /// Where the association is in its lifecycle: one of `DomainPackageStatus`'s
    /// values.
    pub status: String,
    /// The status move the clock owes this association, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// `DomainPackageDetails$PackageVersion`: the version of the package's content
    /// this association put on the domain, which is the version the package had
    /// available when the association was asked for. A package whose content is
    /// updated does not change what a domain already has: a later association is
    /// what moves the domain onto the newer version.
    pub package_version: String,
    /// `DomainPackageDetails$PrerequisitePackageIDList`: the packages this one
    /// needed on the domain, as the call that made the association named them.
    pub prerequisite_package_ids: Vec<String>,
    /// `DomainPackageDetails$AssociationConfiguration`, absent where the call that
    /// made the association carried none.
    pub association_configuration: Option<PackageAssociationConfiguration>,
    /// `DomainPackageDetails$LastUpdated`: "timestamp of the most recent update to
    /// the package association status".
    pub last_updated: SystemTime,
}

impl PackageAssociation {
    /// Materialise the status move the clock owes this association, if `now` is past
    /// its due instant: a package that was being put in place on the domain becomes
    /// one that is in place there.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called — which is why the
    /// record is stamped with the instant the move fell DUE rather than the instant
    /// of the call that happened to observe it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
        self.last_updated = owed.due;
    }

    /// Whether this association still has a status move outstanding, meaning the
    /// change asked for by the call that last touched it has not landed yet.
    ///
    /// Read AFTER [`Self::settle`], so a move that has merely fallen due does not
    /// read as one still in flight.
    pub fn in_flight(&self) -> bool {
        self.owed.is_some()
    }

    /// This record as `DomainPackageDetails` — the shape every operation that
    /// reports an association answers with, so an association's own answer and a
    /// later read of it cannot disagree.
    ///
    /// `package` is the stored record of the package this association links to: its
    /// name and type belong to the PACKAGE, so they are read from there rather than
    /// copied onto the association.
    pub fn to_details(&self, domain_name: &str, package: &Package) -> DomainPackageDetails {
        DomainPackageDetails {
            package_id: Some(self.package_id.clone()),
            package_name: Some(package.package_name.clone()),
            package_type: Some(package.package_type.clone()),
            last_updated: Some(datetime(self.last_updated)),
            domain_name: Some(domain_name.to_string()),
            domain_package_status: Some(self.status.clone()),
            package_version: Some(self.package_version.clone()),
            // "A list of package IDs that must be associated with the domain before
            // or with the package can be associated": reported only where the
            // association has any.
            prerequisite_package_id_list: (!self.prerequisite_package_ids.is_empty())
                .then(|| self.prerequisite_package_ids.clone()),
            reference_path: package_reference_path(package),
            // "Additional information if the package is in an error state. Null
            // otherwise." This association is not in one.
            error_details: None,
            association_configuration: self.association_configuration.clone(),
        }
    }
}

/// `DomainPackageDetails$ReferencePath`: "the relative path of the package on the
/// OpenSearch Service cluster nodes. This is `synonym_path` when the package is for
/// synonym files."
///
/// Dictionary content is the kind a domain's index settings name by path, and the
/// documented form of that reference is the analyzers directory followed by the
/// package's id — so the path is DERIVED from the stored package rather than
/// invented per call. Content of any other type is loaded onto the nodes rather
/// than referenced by an index setting, so it has no such path.
pub fn package_reference_path(package: &Package) -> Option<String> {
    (package.package_type == PACKAGE_TYPE_TXT_DICTIONARY)
        .then(|| format!("{ANALYZERS_DIRECTORY}/{}", package.package_id))
}

/// The `ResourceNotFoundException` payload for an id that names no package of this
/// account — the package mirror of [`domain_not_found`].
pub fn package_not_found(package_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Package not found: {package_id}")),
    }
}

/// The `ConflictException` payload for associating a package whose content is not in
/// place: a package can be put on a domain only once its copy has left it
/// [`PACKAGE_STATUS_AVAILABLE`], so a call made before that conflicts with the
/// status the package is in, and names it.
pub fn package_not_available(package_id: &str, status: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Package {package_id} is {status}, not {PACKAGE_STATUS_AVAILABLE}."
        )),
    }
}

/// The `ConflictException` payload for a call that would change an association which
/// already has a change in flight: the association is in use by that change until it
/// lands, and the refusal names the status it is in.
pub fn association_in_flight(
    package_id: &str,
    domain_name: &str,
    status: &str,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "The association of package {package_id} with domain {domain_name} is {status}."
        )),
    }
}

/// The `ValidationException` payload for a prerequisite that the domain does not
/// have and the call does not associate either.
///
/// A prerequisite "must be linked to the domain before or simultaneously with the
/// package association", so the refusal names both packages and the domain the link
/// is missing on.
pub fn prerequisite_not_associated(
    package_id: &str,
    prerequisite: &str,
    domain_name: &str,
) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "PrerequisitePackageIDList of package {package_id} names {prerequisite}, which is \
             not associated with domain {domain_name} and not associated by this request."
        )),
    }
}

/// The `ValidationException` payload for a package the same call names twice: a
/// domain holds ONE association per package, so a request that names it twice asks
/// for two states of one association and cannot be served as asked.
pub fn package_named_twice(member: &str, package_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} names package {package_id} twice.")),
    }
}

/// The `ValidationException` payload for a package that names ITSELF among the
/// packages that must be associated before it: no association can precede its own.
pub fn prerequisite_is_self(package_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "PrerequisitePackageIDList of package {package_id} names that same package."
        )),
    }
}

/// Per-service state for com-amazonaws-opensearch-amazonopensearchservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Every created domain, keyed by its name — unique per account and Region, so
    /// the name IS the key. Iterating a `BTreeMap` is order-stable, which is what
    /// lets `ListDomainNames` report the same order twice.
    pub domains: BTreeMap<String, Domain>,
    /// Every cross-cluster connection, keyed by its `ConnectionId` — unique per
    /// connection, so the id IS the key. Iterating a `BTreeMap` is order-stable,
    /// which is what lets `DescribeOutboundConnections` report the same order twice
    /// and page through the set without repeating or skipping a record.
    pub connections: BTreeMap<String, Connection>,
    /// Drives [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_connection_id`].
    pub connection_seq: u64,
    /// Drives [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_endpoint_suffix`].
    pub endpoint_suffix_seq: u64,
    /// The Reserved Instance offering catalog this emulator publishes, keyed by
    /// `ReservedInstanceOfferingId`. Seeded by [`seed_reserved_instance_offerings`]
    /// and never written again: an offering is published by the service, so no
    /// operation creates, changes or removes one, and iterating a `BTreeMap` is
    /// order-stable, which is what lets a paged read of the catalog resume where the
    /// previous page stopped.
    pub reserved_instance_offerings: BTreeMap<String, ReservedInstanceOffering>,
    /// The default-application setting: the ARN of the application that is the
    /// account's default, or `None` while no default is set.
    ///
    /// ONE account-scoped value rather than a flag on a per-application record:
    /// `PutDefaultApplicationSetting` sets or clears it and
    /// `GetDefaultApplicationSetting` reports it, so the two cannot disagree about
    /// which application is the default.
    pub default_application_arn: Option<String>,
    /// Every purchased Reserved Instance, keyed by its `ReservedInstanceId` — "the
    /// unique identifier for the reservation", so the id IS the key.
    /// `PurchaseReservedInstanceOffering` mints one and
    /// `DescribeReservedInstances` reports it, from THIS one record.
    pub reserved_instances: BTreeMap<String, ReservedInstanceRecord>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_billing_subscription_id`].
    pub billing_subscription_seq: i64,
    /// Every created package, keyed by its `PackageID` — unique per package, so the
    /// id IS the key. `CreatePackage` mints one and `DescribePackages` reports it,
    /// from THIS one record. Iterating a `BTreeMap` is order-stable, which is what
    /// lets a paged read of the set resume where the previous page stopped without
    /// repeating or skipping a record.
    pub packages: BTreeMap<String, Package>,
}

impl Default for ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// The store a fresh engine hands this service: no account resources at all,
    /// and the published Reserved Instance offering catalog already seeded,
    /// because the catalog is service content rather than something a call
    /// creates.
    fn default() -> Self {
        Self {
            domains: BTreeMap::new(),
            connections: BTreeMap::new(),
            connection_seq: 0,
            endpoint_suffix_seq: 0,
            reserved_instance_offerings: seed_reserved_instance_offerings(),
            default_application_arn: None,
            reserved_instances: BTreeMap::new(),
            billing_subscription_seq: 0,
            packages: BTreeMap::new(),
        }
    }
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

/// The bijection's constant term for a domain endpoint's suffix, so the first suffix
/// minted is not a run of leading `a`s.
const SUFFIX_OFFSET: u128 = 734_291;

/// The bijection's constant term for a connection id. Different from
/// [`SUFFIX_OFFSET`], so the nth connection id and the nth endpoint suffix — minted
/// off two counters that both start at zero — do not carry the same token.
const CONNECTION_ID_OFFSET: u128 = 51_539_607_551;

/// The opaque lowercase-alphanumeric token seed `seq` scrambles to, through the
/// bijection described on [`SUFFIX_MULTIPLIER`]: distinct seeds under one `offset`
/// can never render the same token.
fn opaque_token(seq: u64, offset: u128) -> String {
    let mut scrambled = (u128::from(seq) * SUFFIX_MULTIPLIER + offset) % SUFFIX_SPACE;
    let mut token = Vec::with_capacity(SUFFIX_LEN as usize);
    for _ in 0..SUFFIX_LEN {
        let digit = usize::try_from(scrambled % 36).unwrap_or(0);
        token.push(SUFFIX_ALPHABET[digit]);
        scrambled /= 36;
    }
    String::from_utf8(token).unwrap_or_default()
}

impl ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Mint the opaque suffix a domain's endpoint host carries, so two domains of
    /// the same name in different runs do not share an endpoint.
    ///
    /// Derived from a counter through the bijection described on
    /// [`SUFFIX_MULTIPLIER`], so it is unpredictable-looking yet never repeats.
    pub fn mint_endpoint_suffix(&mut self) -> String {
        self.endpoint_suffix_seq = self.endpoint_suffix_seq.wrapping_add(1);
        opaque_token(self.endpoint_suffix_seq, SUFFIX_OFFSET)
    }

    /// Mint the identifier a cross-cluster connection is addressed by, so two
    /// connections never share one.
    ///
    /// `ConnectionId` is `@length(min: 10, max: 256)` with pattern
    /// `^[a-z][a-z0-9\-]+$` — an opaque lowercase token. It is minted off its own
    /// counter through the same bijection as [`Self::mint_endpoint_suffix`], behind a
    /// `cc-` prefix for the cross-cluster resource it names, which is also what
    /// satisfies the pattern's leading letter and carries the token past the
    /// 10-character minimum.
    pub fn mint_connection_id(&mut self) -> String {
        self.connection_seq = self.connection_seq.wrapping_add(1);
        format!(
            "cc-{}",
            opaque_token(self.connection_seq, CONNECTION_ID_OFFSET)
        )
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

    /// Materialise the status move the clock owes every stored cross-cluster
    /// connection, for a caller about to report on all of them.
    pub fn settle_connections(&mut self, now: SystemTime) {
        for connection in self.connections.values_mut() {
            connection.settle(now);
        }
    }

    /// Materialise the status move the clock owes every stored package, for a caller
    /// about to report on all of them.
    pub fn settle_packages(&mut self, now: SystemTime) {
        for package in self.packages.values_mut() {
            package.settle(now);
        }
    }

    /// The package `package_name` names, if the account holds one.
    ///
    /// The store is keyed by `PackageID`, but a package NAME is unique across the
    /// account's packages, so this is the one lookup every operation that resolves a
    /// name goes through.
    pub fn package_named(&self, package_name: &str) -> Option<&Package> {
        self.packages
            .values()
            .find(|package| package.package_name == package_name)
    }

    /// Store `package` and hand back the stored copy, so a create answers with the
    /// record this service persisted rather than a copy beside it.
    pub fn insert_package(&mut self, package: Package) -> &Package {
        let id = package.package_id.clone();
        self.packages.entry(id).or_insert(package)
    }

    /// The published offering whose `ReservedInstanceOfferingId` is `offering_id`.
    ///
    /// The catalog is keyed by that id, so this is the one lookup both the
    /// offerings listing and a purchase resolve an offering by.
    pub fn reserved_instance_offering(
        &self,
        offering_id: &str,
    ) -> Option<&ReservedInstanceOffering> {
        self.reserved_instance_offerings.get(offering_id)
    }

    /// Mint the `BillingSubscriptionId` a purchase is billed under: "the unique
    /// identifier of the billing subscription".
    ///
    /// A counter, so no two purchases share one and the id is stable for the
    /// reservation it was minted for.
    pub fn mint_billing_subscription_id(&mut self) -> i64 {
        self.billing_subscription_seq = self.billing_subscription_seq.saturating_add(1);
        self.billing_subscription_seq
    }

    /// The number of OpenSearch instances this account currently has reserved, which
    /// is what [`RESERVED_INSTANCE_QUOTA`] bounds.
    pub fn reserved_instance_count(&self) -> i64 {
        self.reserved_instances
            .values()
            .map(|reservation| i64::from(reservation.instance_count))
            .sum()
    }

    /// Store `record` and hand back the stored copy, so the purchase answers with
    /// the record this service persisted rather than a copy beside it.
    pub fn insert_reserved_instance(
        &mut self,
        record: ReservedInstanceRecord,
    ) -> &ReservedInstanceRecord {
        let id = record.reserved_instance_id.clone();
        self.reserved_instances.insert(id.clone(), record);
        &self.reserved_instances[&id]
    }

    /// Materialise every state change the clock owes the stored reservations, for a
    /// caller about to report on them.
    pub fn settle_reserved_instances(&mut self, now: SystemTime) {
        for reservation in self.reserved_instances.values_mut() {
            reservation.settle(now);
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
    new_guid()
}

/// A fresh `GUID`, for whichever resource this service addresses by one; see
/// [`new_change_id`] for why that is a v4 UUID.
fn new_guid() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The `ReservedInstanceId` a purchase mints: the unique identifier for the
/// reservation, a `GUID` like every other id this service hands out
/// ([`new_guid`]).
pub fn new_reserved_instance_id() -> String {
    new_guid()
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

/// The `ValidationException` payload for a member that must be a `GUID` and is not.
///
/// It names the member and the shape the value had to have, so a caller can fix the
/// request from the refusal alone.
pub fn malformed_guid(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must be a GUID of the form xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx: {value}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for an offering id the catalog publishes
/// no Reserved Instance offering for.
pub fn offering_not_found(offering_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Reserved Instance offering not found: {offering_id}"
        )),
    }
}

/// The `ValidationException` payload for a pagination token that does not name a
/// place in the listing it was sent to.
///
/// A token is only ever a value the service itself handed out, so one it cannot
/// resolve is a malformed request rather than an empty page.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: Some(
            "NextToken is not a pagination token this operation issued. Send the NextToken \
             returned by the previous page, or omit it to start from the first page."
                .to_string(),
        ),
    }
}

/// The number of colon-separated fields an ARN carries:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`. The resource field
/// may itself contain colons, so it is the LAST field and is not split further.
const ARN_FIELDS: usize = 6;

/// Whether `value` has ARN syntax: the `arn` literal, a partition, a service and a
/// non-empty resource, with the Region and account-id fields allowed to be empty
/// (service-wide and account-less ARNs leave them so).
///
/// The `ARN` shape is `@length(min: 20, max: 2048)` with pattern `.*`, so the request
/// boundary enforces only the length: an operation that takes an ARN checks the
/// syntax itself rather than storing a value no ARN parser would accept — the same
/// reasoning as [`is_guid`]. It is a SYNTAX test only: resolving an ARN to a stored
/// resource is the job of whichever registry owns that resource.
pub fn is_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.splitn(ARN_FIELDS, ':').collect();
    fields.len() == ARN_FIELDS
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && !fields[2].is_empty()
        && !fields[5].is_empty()
}

/// The `ValidationException` payload for a member that must carry an ARN and does
/// not. It names the member and the value that could not be read as one, so the
/// caller can tell which input it was.
pub fn malformed_arn(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} is not a valid ARN: {value}")),
    }
}

/// The `ResourceNotFoundException` payload for a read of the default-application
/// setting made while no default application is set — the state the setting is in
/// before any `PutDefaultApplicationSetting`, and after one that cleared it.
pub fn default_application_not_set() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("No default application is set for this account.".to_string()),
    }
}

/// The `ResourceAlreadyExistsException` payload for a create naming a connection
/// alias the source domain already has a connection under — the mirror of
/// [`data_source_exists`], so one refusal reads like the other.
pub fn connection_alias_exists(alias: &str, local_domain: &str) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Domain {local_domain} already has a cross-cluster connection with the alias {alias}."
        )),
    }
}

/// The `InvalidPaginationTokenException` payload for a `NextToken` that names no
/// place in the set being paged: one this service did not mint, or one whose record
/// has since been released.
///
/// The same refusal as [`invalid_next_token`] in a different modelled shape: an
/// operation that LISTS `InvalidPaginationTokenException` refuses an unresolvable
/// token with that error, and one that does not falls back to its
/// `ValidationException`.
pub fn invalid_pagination_token(token: &str) -> InvalidPaginationTokenException {
    InvalidPaginationTokenException {
        message: Some(format!("Invalid pagination token: {token}")),
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

// ---------------------------------------------------------------------------
// Reserved Instances
//
// A Reserved Instance IS an account resource, unlike the offering it is bought
// from: `PurchaseReservedInstanceOffering` mints one from a catalog entry above
// and `DescribeReservedInstances` reports it, from the one record below.
// ---------------------------------------------------------------------------

/// The number of instances a purchase reserves when the request names none.
///
/// `PurchaseReservedInstanceOffering$InstanceCount` is optional with
/// `@range(min: 1)`, so a purchase that names no count reserves the fewest it
/// could have named: one instance.
pub const DEFAULT_INSTANCE_COUNT: i32 = 1;

/// The states a reservation is reported in.
///
/// `ReservedInstance$State` is "the state of the Reserved Instance" and the model
/// declares no enum for it, so these are the states a reserved-instance purchase
/// passes through: it is `payment-pending` until its payment settles, `active`
/// for the term it was bought for, and `retired` once that term has elapsed.
pub const RESERVATION_STATE_PAYMENT_PENDING: &str = "payment-pending";
/// See [`RESERVATION_STATE_PAYMENT_PENDING`].
pub const RESERVATION_STATE_ACTIVE: &str = "active";
/// See [`RESERVATION_STATE_PAYMENT_PENDING`].
pub const RESERVATION_STATE_RETIRED: &str = "retired";

/// How long a reservation stays `payment-pending` before the clock owes it
/// `active`.
///
/// The same reasoning as [`PROVISIONING`]: AWS takes as long as the payment
/// takes, the emulator chooses a span, and it is deliberately NOT zero so a
/// caller describing straight after the purchase still observes the transient
/// state. The due instant is stored on the record and materialised by whichever
/// later call reads it — nothing sleeps.
pub const RESERVATION_SETTLEMENT: Duration = Duration::from_millis(500);

/// How many OpenSearch instances this account may hold reserved before
/// `PurchaseReservedInstanceOffering` refuses with the modelled
/// `LimitExceededException`.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it
/// models neither Service Quotas nor an account's adjustments — and it is what
/// stops a caller looping with fresh reservation names from growing the state
/// (and the describe's answer) without end.
pub const RESERVED_INSTANCE_QUOTA: i64 = 100;

/// The values a new reservation carries that do NOT come from the offering it is
/// bought from — one argument, so [`ReservedInstanceRecord::from_offering`] does
/// not take a row of positional values.
#[derive(Debug, Clone)]
pub struct NewReservation {
    /// The `GUID` the reservation is addressed by, from
    /// [`new_reserved_instance_id`].
    pub reserved_instance_id: String,
    /// `ReservationName`: "the customer-specified identifier to track this
    /// reservation".
    pub reservation_name: String,
    /// `InstanceCount`: how many instances the purchase reserved.
    pub instance_count: i32,
    /// `BillingSubscriptionId`, from
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_billing_subscription_id`].
    pub billing_subscription_id: i64,
    /// The instant of the purchase, which is the reservation's `StartTime`.
    pub now: SystemTime,
}

/// One Reserved Instance, as this emulator persists it.
///
/// Every priced member is COPIED out of the catalog entry the reservation was
/// bought from, which is what makes a reservation a record of what WAS bought
/// rather than a second view of the live catalog.
#[derive(Debug, Clone)]
pub struct ReservedInstanceRecord {
    pub reservation_name: String,
    /// "The unique identifier for the reservation", which is this record's
    /// identity in the store.
    pub reserved_instance_id: String,
    pub billing_subscription_id: i64,
    /// The offering it was bought from.
    pub reserved_instance_offering_id: String,
    pub instance_type: Option<String>,
    /// "The date and time when the reservation was purchased."
    pub start_time: SystemTime,
    /// "The duration, in seconds, for which the OpenSearch instance is
    /// reserved", from the offering's term.
    pub duration_seconds: i32,
    pub fixed_price: Option<f64>,
    pub usage_price: Option<f64>,
    pub currency_code: Option<String>,
    /// "The number of OpenSearch instances that have been reserved."
    pub instance_count: i32,
    /// One of [`RESERVATION_STATE_PAYMENT_PENDING`],
    /// [`RESERVATION_STATE_ACTIVE`] or [`RESERVATION_STATE_RETIRED`].
    pub state: String,
    /// When the clock owes this reservation its move out of `payment-pending`;
    /// `None` once that move has been materialised.
    pub active_due: Option<SystemTime>,
    pub payment_option: Option<String>,
    /// "The recurring charge to your account, regardless of whether you create
    /// any domains using the Reserved Instance offering", from the offering.
    pub recurring_charges: Vec<RecurringCharge>,
}

impl ReservedInstanceRecord {
    /// The reservation `new` buys from `offering`.
    pub fn from_offering(offering: &ReservedInstanceOffering, new: NewReservation) -> Self {
        Self {
            reservation_name: new.reservation_name,
            reserved_instance_id: new.reserved_instance_id,
            billing_subscription_id: new.billing_subscription_id,
            reserved_instance_offering_id: offering
                .reserved_instance_offering_id
                .clone()
                .unwrap_or_default(),
            instance_type: offering.instance_type.clone(),
            start_time: new.now,
            duration_seconds: offering.duration,
            // The offering's prices are per reserved instance — "the upfront
            // fixed charge you will paid to purchase the specific Reserved
            // Instance offering" and "the hourly rate at which you're charged
            // for the domain using this Reserved Instance" — so a multi-instance
            // reservation reports the same per-instance prices beside its
            // `InstanceCount` rather than a total no member of this shape
            // documents.
            fixed_price: offering.fixed_price,
            usage_price: offering.usage_price,
            currency_code: offering.currency_code.clone(),
            instance_count: new.instance_count,
            state: RESERVATION_STATE_PAYMENT_PENDING.to_string(),
            active_due: Some(new.now + RESERVATION_SETTLEMENT),
            payment_option: offering.payment_option.clone(),
            recurring_charges: offering.recurring_charges.clone().unwrap_or_default(),
        }
    }

    /// Materialise every state change the clock owes this reservation — the only
    /// write a reporting operation makes, and only what was already owed at this
    /// instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_payment(now);
        self.settle_term(now);
    }

    /// `payment-pending` becomes `active` once the payment's due instant has
    /// passed.
    fn settle_payment(&mut self, now: SystemTime) {
        let Some(due) = self.active_due else {
            return;
        };
        if now < due {
            return;
        }
        self.state = RESERVATION_STATE_ACTIVE.to_string();
        self.active_due = None;
    }

    /// An `active` reservation becomes `retired` once the term it was bought for
    /// has elapsed: it runs for its offering's `Duration` from its `StartTime`,
    /// and that span is the offering's own rather than one the emulator chose.
    fn settle_term(&mut self, now: SystemTime) {
        if self.state != RESERVATION_STATE_ACTIVE {
            return;
        }
        let Ok(seconds) = u64::try_from(self.duration_seconds) else {
            return;
        };
        if now < self.start_time + Duration::from_secs(seconds) {
            return;
        }
        self.state = RESERVATION_STATE_RETIRED.to_string();
    }

    /// This record as the modelled `ReservedInstance`.
    pub fn to_modeled(&self) -> ReservedInstance {
        ReservedInstance {
            reservation_name: Some(self.reservation_name.clone()),
            reserved_instance_id: Some(self.reserved_instance_id.clone()),
            billing_subscription_id: Some(self.billing_subscription_id),
            reserved_instance_offering_id: Some(self.reserved_instance_offering_id.clone()),
            instance_type: self.instance_type.clone(),
            start_time: Some(datetime(self.start_time)),
            duration: self.duration_seconds,
            fixed_price: self.fixed_price,
            usage_price: self.usage_price,
            currency_code: self.currency_code.clone(),
            instance_count: self.instance_count,
            state: Some(self.state.clone()),
            payment_option: self.payment_option.clone(),
            recurring_charges: Some(self.recurring_charges.clone()),
        }
    }
}

/// The refusal of a `GUID` that names no stored Reserved Instance.
///
/// The modelled `ValidationException` (HTTP 400) rather than
/// `ResourceNotFoundException`: both are documented in this model with the very
/// same sentence — "an exception for accessing or deleting a resource that
/// doesn't exist" — and this is the refusal the service answers an identifier
/// that resolves to no reservation with.
pub fn reserved_instance_not_found(reserved_instance_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Reserved instance not found: {reserved_instance_id}"
        )),
    }
}
