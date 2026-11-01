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
//! `CreateOutboundConnection` mints one, `DescribeOutboundConnections` reports it
//! to the source domain's owner and `DescribeInboundConnections` reports it to the
//! destination domain's owner. One record carries both views of a connection — the
//! source domain owner's outbound one and the destination owner's inbound one —
//! because they are one resource sharing a `ConnectionId`.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

/// Whether IAM lets a request perform the action it asks for — the authorization step
/// an index operation takes before it reads or writes the index the request names.
pub mod authorization;
pub mod ingest;

/// Fine-grained access control on a domain: the records its security API holds, and the
/// one decision every request that reaches the domain is taken by.
pub mod fgac;

/// Whether the Amazon Cognito authentication a request asks a domain's Dashboards to be
/// reached through is a configuration the service can provision — the check the create
/// path and the update path share.
pub mod cognito_authentication;

/// Whether a value satisfies its member's modelled `@pattern` over its WHOLE length —
/// the reading AWS applies, which the request boundary's find does not enforce for an
/// unanchored expression.
mod whole_value_pattern;

use crate::ops::types::{
    AIMLOptionsInput, AIMLOptionsOutput, AIMLOptionsStatus, AWSDomainInformation,
    AccessPoliciesStatus, AdditionalLimit,
    AdvancedOptionsStatus, AdvancedSecurityOptions, AdvancedSecurityOptionsInput,
    AdvancedSecurityOptionsStatus, AppConfig, ApplicationSummary, AuthorizedPrincipal,
    AutoTune, AutoTuneDetails,
    AutoTuneOptions, AutoTuneOptionsInput, AutoTuneOptionsOutput,
    AutoTuneOptionsStatus, AutoTuneStatus, AutomatedSnapshotPauseOptions,
    AutomatedSnapshotPauseOptionsStatus, AutomatedSnapshotPauseRequestOptions, BaseException,
    ChangeProgressDetails, ChangeProgressStatusDetails, ClusterConfig, ClusterConfigStatus,
    CognitoOptions, CognitoOptionsStatus, ConflictException, ConnectionProperties,
    DataSource as LinkedDataSource, DataSourceAttachmentSummary, DataSourceDetails, DataSourceType,
    DeploymentStrategyOptions,
    DeploymentStrategyOptionsStatus, DependencyFailureException, DirectQueryDataSource,
    DirectQueryDataSourceType,
    DisabledOperationException,
    DomainConfig, DomainEndpointOptions,
    DomainEndpointOptionsStatus, DomainInfo, DomainInformationContainer, DomainMaintenanceDetails,
    DomainPackageDetails,
    DomainStatus, EBSOptions,
    EBSOptionsStatus,
    DryRunProgressStatus, DryRunResults,
    EncryptionAtRestOptions, EncryptionAtRestOptionsStatus, EngineModeStatus, ErrorDetails,
    ExportOptions,
    IAMFederationOptionsOutput, IPAddressTypeStatus, IamIdentityCenterOptions,
    IamIdentityCenterOptionsInput, IdentityCenterOptions,
    IdentityCenterOptionsInput, IdentityCenterOptionsStatus, InboundConnection,
    InboundConnectionStatus, Insight, InsightEntity, InsightFeedbackEntity, InsightField,
    InstanceCountLimits, InstanceLimits,
    InstanceTypeDetails, InternalException,
    InvalidPaginationTokenException, InvalidTypeException, JWTOptionsOutput,
    LimitExceededException, Limits,
    LogPublishingOption,
    LogPublishingOptionsStatus, MigrationSource, MigrationSummary, MigrationWorkspace,
    NaturalLanguageQueryGenerationOptionsOutput,
    NodeToNodeEncryptionOptions, NodeToNodeEncryptionOptionsStatus, OffPeakWindowOptions,
    OffPeakWindowOptionsStatus, OptionStatus, OutboundConnection, OutboundConnectionStatus,
    PackageAssociationConfiguration, PackageConfiguration, PackageDetails,
    PackageEncryptionOptions, PackageSource,
    PackageVendingOptions, PackageVersionHistory,
    RecurringCharge, ReservedInstance,
    ReservedInstanceOffering, ResourceAlreadyExistsException, ResourceNotFoundException,
    RollbackServiceSoftwareOptions, SAMLOptionsOutput, ScheduledAction,
    ScheduledAutoTuneDetails, ServiceSoftwareOptions,
    SnapshotOptions,
    SnapshotOptionsStatus, SoftwareUpdateOptions, SoftwareUpdateOptionsStatus, StorageType,
    StorageTypeLimit, Tag, UpgradeHistory, UpgradeStepItem, UseCaseStatus,
    VPCDerivedInfo, VPCDerivedInfoStatus, VPCOptions, ValidationException, VersionStatus,
    VpcEndpoint, VpcEndpointError, VpcEndpointSummary,
    INBOUND_CONNECTION_STATUS_CODE_VALUES,
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES, RESERVED_INSTANCE_PAYMENT_OPTION_VALUES,
    SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES, SCHEDULE_AT_VALUES,
    VOLUME_TYPE_VALUES,
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
/// is what lets two reads report the same versions in the same order. The order a
/// caller is SHOWN the catalog in is [`published_engine_versions`]'s, which is this
/// set the other way round.
pub const SUPPORTED_ENGINE_VERSIONS: [&str; 36] = [
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
    "OpenSearch_3.1",
    "OpenSearch_3.3",
    "OpenSearch_3.5",
    "OpenSearch_3.7",
];

/// The published catalog in the order the service PUBLISHES it: newest release first,
/// so the OpenSearch line leads and each line's releases descend by major then minor,
/// down to the oldest Elasticsearch release.
///
/// A catalog of supported releases is read top-down for the CURRENT release, so that is
/// the end it is published from; [`SUPPORTED_ENGINE_VERSIONS`] is written the other way
/// round because the upgrade relation is defined along it. This is the ONE reversal of
/// that set, derived from [`upgrade_rank`] rather than written out a second time, so the
/// catalog a caller lists and the catalog a version is resolved against stay one set.
pub fn published_engine_versions() -> Vec<&'static str> {
    let mut versions = SUPPORTED_ENGINE_VERSIONS.to_vec();
    versions.sort_by_key(|version| ::std::cmp::Reverse(upgrade_rank(version)));
    versions
}

/// The engine version a domain runs when `CreateDomain` names none.
///
/// `EngineVersion` is optional and the service picks a current OpenSearch version
/// when it is omitted. It is one of [`SUPPORTED_ENGINE_VERSIONS`], so a domain
/// created without one runs a version the service publishes.
pub const DEFAULT_ENGINE_VERSION: &str = "OpenSearch_2.11";

/// Whether the service OFFERS `engine_version`: whether it is one of the versions
/// [`SUPPORTED_ENGINE_VERSIONS`] publishes, which is the set `ListVersions`
/// advertises.
///
/// `VersionString`'s pattern admits every `Elasticsearch_X.Y` and `OpenSearch_X.Y`
/// value, so being well formed does not make a value a release the service has:
/// which releases exist is this catalog's answer, not the pattern's.
pub fn offers_engine_version(engine_version: &str) -> bool {
    SUPPORTED_ENGINE_VERSIONS.contains(&engine_version)
}

/// The generation [`upgrade_rank`] ranks a legacy Elasticsearch release at.
const ENGINE_GENERATION_ELASTICSEARCH: u8 = 0;

/// The generation it ranks an OpenSearch release at: the engine line that SUCCEEDS the
/// Elasticsearch one.
const ENGINE_GENERATION_OPENSEARCH: u8 = 1;

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
        ENGINE_TYPE_ELASTICSEARCH => ENGINE_GENERATION_ELASTICSEARCH,
        ENGINE_TYPE_OPENSEARCH => ENGINE_GENERATION_OPENSEARCH,
        // An engine neither constant names has no place in the order rather than
        // sharing the newest line's.
        _ => return None,
    };
    let (major, minor) = engine_version.split_once('_')?.1.split_once('.')?;
    Some((generation, major.parse().ok()?, minor.parse().ok()?))
}

/// The major release number of the LAST Elasticsearch line, the one OpenSearch forked
/// from.
const ELASTICSEARCH_FORK_MAJOR: u32 = 7;

/// The major release number of the FIRST OpenSearch line, the fork itself.
const OPENSEARCH_FORK_MAJOR: u32 = 1;

/// A version's UPGRADE LINE: the engine generation it belongs to and its MAJOR release
/// number — the `(0, 7)` of `Elasticsearch_7.10` — which is the unit an upgrade path is
/// published along.
///
/// The Elasticsearch 7 releases and the OpenSearch 1 releases are ONE such line, because
/// OpenSearch 1.0 is the fork of Elasticsearch 7.10: a domain moves from one to the other
/// the way it moves between two minors of one major, without the service treating it as a
/// major-line change. `None` for a value [`upgrade_rank`] cannot read.
fn upgrade_line(engine_version: &str) -> Option<(u8, u32)> {
    let (generation, major, _) = upgrade_rank(engine_version)?;
    if (generation, major) == (ENGINE_GENERATION_OPENSEARCH, OPENSEARCH_FORK_MAJOR) {
        return Some((ENGINE_GENERATION_ELASTICSEARCH, ELASTICSEARCH_FORK_MAJOR));
    }
    Some((generation, major))
}

/// Every upgrade line [`SUPPORTED_ENGINE_VERSIONS`] publishes, oldest first and each
/// once: the Elasticsearch majors, then the OpenSearch ones.
fn published_upgrade_lines() -> Vec<(u8, u32)> {
    let mut lines: Vec<(u8, u32)> = SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter_map(|version| upgrade_line(version))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// The upgrade line that immediately SUCCEEDS `line` among the published ones, or
/// `None` for the newest line, which nothing succeeds.
fn succeeding_upgrade_line(line: (u8, u32)) -> Option<(u8, u32)> {
    published_upgrade_lines()
        .into_iter()
        .find(|candidate| *candidate > line)
}

/// Whether `source_version` is the NEWEST release its upgrade line publishes: the one
/// release of a line an upgrade INTO the next line is offered from.
///
/// A version the catalog does not publish at all is the newest of its line when nothing
/// published on that line is later than it, so a domain running an unpublished release
/// still reports the path ahead of it.
fn is_newest_of_upgrade_line(source_version: &str) -> bool {
    let Some(source) = upgrade_rank(source_version) else {
        return false;
    };
    let Some(line) = upgrade_line(source_version) else {
        return false;
    };
    SUPPORTED_ENGINE_VERSIONS.iter().all(|candidate| {
        upgrade_line(candidate) != Some(line) || upgrade_rank(candidate) <= Some(source)
    })
}

/// The versions a domain running `source_version` can be upgraded to, oldest first:
/// every published release of its OWN upgrade line strictly later than it, plus — from
/// the NEWEST release of that line only — every published release of the line that
/// immediately succeeds it.
///
/// So an upgrade walks the minors of a line freely and leaves the line only from its end:
/// a domain in the middle of a line reaches the end of that line but not past it, and the
/// release at the end reaches the whole of the next line and nothing beyond it. It only
/// ever goes forwards, so a version is never among its own targets and a release at the
/// end of the newest line has none. The answer is derived from [`upgrade_rank`]'s
/// comparison rather than from the source being published, so a domain running a version
/// this emulator does not publish still reports the published versions ahead of it.
pub fn compatible_target_versions(source_version: &str) -> Vec<String> {
    let Some(source) = upgrade_rank(source_version) else {
        return Vec::new();
    };
    let Some(line) = upgrade_line(source_version) else {
        return Vec::new();
    };
    // The crossing is published from the end of a line, so a source with later releases
    // of its own line ahead of it reaches those and stops there.
    let next_line = is_newest_of_upgrade_line(source_version)
        .then(|| succeeding_upgrade_line(line))
        .flatten();
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|candidate| {
            let Some(rank) = upgrade_rank(candidate) else {
                return false;
            };
            let candidate_line = upgrade_line(candidate);
            (candidate_line == Some(line) && rank > source)
                || (next_line.is_some() && candidate_line == next_line)
        })
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The oldest release the service still offers an UPGRADE from, which is the floor of
/// the published upgrade table: a release older than this one is still supported to run
/// — [`SUPPORTED_ENGINE_VERSIONS`] publishes it and a domain can be created on it — but
/// the service publishes no upgrade path out of it.
///
/// The model carries no record of which releases an upgrade is offered from, so this is
/// the figure the SERVICE publishes: an in-place upgrade is offered for a domain running
/// Elasticsearch 5.1 or later, and for an older release the service publishes no upgrade
/// path at all. What the table then guarantees is that it has a floor and that every
/// release at or above it with somewhere to go is listed.
pub const UPGRADE_OFFERED_FROM: &str = "Elasticsearch_5.1";

/// The source versions the published upgrade table lists, oldest first: the published
/// releases at or after [`UPGRADE_OFFERED_FROM`] that have at least one target, since a
/// table of "the versions you can upgrade to" has a row only where there is somewhere to
/// go.
pub fn upgradable_source_versions() -> Vec<&'static str> {
    let floor = upgrade_rank(UPGRADE_OFFERED_FROM);
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .copied()
        .filter(|version| {
            upgrade_rank(version) >= floor && !compatible_target_versions(version).is_empty()
        })
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

/// `IPAddressType`'s `dualstack` value — the other being `ipv4`
/// (`smithy_query(query="shape",shape="IPAddressType")`). A domain set to it is
/// reachable over IPv6 as well as IPv4, which is what gives it a second, dual-stack
/// host beside its IPv4 one.
pub const IP_ADDRESS_TYPE_DUALSTACK: &str = "dualstack";

/// The `DomainStatus$Endpoints` key a VPC-access domain's IPv4 host is published
/// under, from that member's own documentation: "IPv4 IP addresses -
/// `'vpc','vpc-endpoint-<opaque-id>.<region>.es.amazonaws.com'`".
pub const ENDPOINTS_KEY_VPC: &str = "vpc";

/// The `DomainStatus$Endpoints` key a VPC-access domain's DUAL-STACK host is
/// published under, from that member's own documentation: "Dual stack IP addresses -
/// `'vpcv2':'vpc-endpoint-<opaque-id>.aos.<region>.on.aws'`".
pub const ENDPOINTS_KEY_VPC_DUALSTACK: &str = "vpcv2";

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

/// How many tags ONE taggable resource carries before a call that would attach
/// another is refused with the modelled `LimitExceededException`.
///
/// Documented by the service rather than chosen here: "A domain, data source, or
/// application can have up to 10 tags"
/// (`smithy_query(query="doc",shape="AddTags")`).
pub const TAG_QUOTA: usize = 10;

/// A taggable resource's tag set: the `TagValue` each `TagKey` carries.
///
/// A map rather than a list of the modelled `Tag`, because "Tag keys must be unique
/// for the domain to which they are attached" (`Tag$Key`) — keying the store by the
/// tag key is what makes that uniqueness structural instead of something each
/// handler has to re-enforce. Iterating a `BTreeMap` is order-stable, which is what
/// lets two reads of one resource's tags report them in the same order.
pub type TagSet = BTreeMap<String, String>;

/// The stored form of a modelled `TagList`: the last value a repeated key carries is
/// the one the resource holds, because the set holds ONE value per key.
pub fn tag_set(tag_list: Vec<Tag>) -> TagSet {
    tag_list
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

/// A stored tag set as the modelled `TagList` a response carries.
pub fn tag_list(tags: &TagSet) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The `LimitExceededException` payload for a call that would leave a resource
/// carrying more than [`TAG_QUOTA`] tags.
pub fn tag_quota_exceeded(arn: &str, requested: usize) -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "{arn} would carry {requested} tags, more than the {TAG_QUOTA} a resource may have."
        )),
    }
}

/// The same bound as [`tag_quota_exceeded`], reported through the client-input error
/// channel for a CREATE whose model declares no `LimitExceededException`: the resource
/// does not exist yet, so the refusal names the member that carried the tags rather
/// than an ARN nothing has been minted under.
pub fn tag_quota_exceeded_on_create(member: &str, requested: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} carries {requested} tags, more than the {TAG_QUOTA} a resource may have."
        )),
    }
}

/// The refusal every tagging operation answers with when its `ARN` resolves to no
/// resource of this account: `ValidationException`, "an exception for accessing or
/// deleting a resource that doesn't exist", which is the ONE not-found error all
/// three tagging operations model (`AddTags` and `RemoveTags` declare no
/// `ResourceNotFoundException` at all, so the reader cannot answer a miss with an
/// error the writers of the same tag set have no way to report).
///
/// The wording is fixed text with no variable part: the ARN resolved to nothing, so
/// there is no resource whose name could fill one in.
pub fn tagged_resource_absent() -> ValidationException {
    ValidationException {
        message: Some("Invalid ARN. Domain not found.".to_string()),
    }
}

/// How long work this control plane starts takes before the clock owes its outcome:
/// an outbound cross-cluster connection stays `VALIDATING` for this long before it
/// owes the next status of its lifecycle, a package stays `COPYING` for this long
/// before it owes the status its copy left it in, and a VPC endpoint, an application
/// and a saved-object migration each move along their own lifecycle on it.
///
/// OpenSearch Service takes many minutes; the emulator chooses a span, and it is
/// deliberately NOT zero so a caller acting straight after the call still observes
/// the transient state and the states that go with it. The due instant is stored on
/// the record and materialised by whichever later call reads it — nothing sleeps.
///
/// Work on the DOMAIN itself has its own, longer span ([`DOMAIN_WORK`]).
pub const PROVISIONING: Duration = Duration::from_millis(500);

/// How long work on a DOMAIN takes before the clock owes its outcome: the
/// provisioning a `CreateDomain` starts, a configuration change an
/// `UpdateDomainConfig` started, an engine version upgrade, and a service software
/// deployment.
///
/// Longer than [`PROVISIONING`] because of what this window is FOR: a domain with
/// work in flight ([`Domain::work_in_flight`]) refuses the calls that would report
/// its nodes or start further work on it, and that refusal is what serializes a
/// client's own work — it issues a change, is told one is in progress, and waits. The
/// window therefore has to stay open for as long as a client takes to make its next
/// call and read the answer, which a span of a few hundred milliseconds does not.
/// OpenSearch Service takes many minutes.
pub const DOMAIN_WORK: Duration = Duration::from_secs(3);

/// How long a change to one domain's package associations takes before the clock owes
/// its outcome: an association stays `ASSOCIATING` for this long before it owes the
/// status being put in place left it in, and one a dissociation has asked to remove
/// stays `DISSOCIATING` for this long before it owes its removal — or the failure that
/// keeps it on the domain.
///
/// Longer than [`PROVISIONING`], because the window it opens is what SERIALIZES
/// writers: the service runs one
/// such change per domain at a time, so a client that issues a second associate or
/// dissociate for the domain right after the call that started a change — on another
/// thread, or after building a second client — must still find that change in flight
/// and be refused rather than overtaking it. OpenSearch Service takes minutes.
pub const PACKAGE_ASSOCIATION_CHANGE: Duration = Duration::from_millis(750);

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

/// `DomainProcessingStatusType`'s `UpgradingEngineVersion`: the domain is being moved
/// onto another engine version.
pub const PROCESSING_STATUS_UPGRADING_ENGINE_VERSION: &str = "UpgradingEngineVersion";

/// `DeploymentStatus`'s `COMPLETED` value: no service software deployment is in
/// flight on the domain.
pub const DEPLOYMENT_STATUS_COMPLETED: &str = "COMPLETED";

/// `DeploymentStatus`'s `IN_PROGRESS` value: a service software deployment is being
/// applied.
pub const DEPLOYMENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `DeploymentStatus`'s `ELIGIBLE` value: a service software release the domain can
/// be moved onto is available.
pub const DEPLOYMENT_STATUS_ELIGIBLE: &str = "ELIGIBLE";

/// `DeploymentStatus`'s `PENDING_UPDATE` value: a deployment has been requested for a
/// later instant and has not begun yet.
pub const DEPLOYMENT_STATUS_PENDING_UPDATE: &str = "PENDING_UPDATE";

/// `OverallChangeStatus`'s `PROCESSING` value: the configuration change is being
/// applied to the domain.
pub const CHANGE_STATUS_PROCESSING: &str = "PROCESSING";

/// `OverallChangeStatus`'s `COMPLETED` value: the configuration change has landed.
pub const CHANGE_STATUS_COMPLETED: &str = "COMPLETED";

/// `OverallChangeStatus`'s `FAILED` value: the configuration change did not land.
///
/// What a CANCELLED change's overall status is: `OverallChangeStatus` publishes only
/// `PENDING`, `PROCESSING`, `COMPLETED` and `FAILED`, so a change that was stopped
/// before it landed is not `COMPLETED` — which of the two it is is said exactly by
/// `ConfigChangeStatus`, whose own [`CONFIG_CHANGE_CANCELLED`] value it carries
/// beside this one.
pub const CHANGE_STATUS_FAILED: &str = "FAILED";

/// `ConfigChangeStatus`'s `ApplyingChanges` value.
pub const CONFIG_CHANGE_APPLYING_CHANGES: &str = "ApplyingChanges";

/// `ConfigChangeStatus`'s `Completed` value.
pub const CONFIG_CHANGE_COMPLETED: &str = "Completed";

/// `ConfigChangeStatus`'s `Cancelled` value: the change was cancelled before it
/// landed, so the domain kept the configuration it was changing from.
pub const CONFIG_CHANGE_CANCELLED: &str = "Cancelled";

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
// Direct query data sources
//
// A direct query data source belongs to the ACCOUNT rather than to a domain:
// it is addressed by its own name ("a unique, user-defined label to identify the
// data source within your OpenSearch Service environment") on paths that name no
// domain at all. `AddDirectQueryDataSource` mints one and
// `GetDirectQueryDataSource` reports it, both from the ONE
// [`DirectQueryDataSourceRecord`] below, so neither can report a source the other
// does not hold.
// ---------------------------------------------------------------------------

/// How many direct query data sources ONE account holds before
/// `AddDirectQueryDataSource` refuses with the modelled `LimitExceededException`.
///
/// Like [`DATA_SOURCE_QUOTA`] this is the emulator's OWN bound on the store — it
/// models neither Service Quotas nor an account's adjustments — and it is what stops
/// a caller looping with fresh names from growing the state (or a listing's answer)
/// without end: every record it counts is released by
/// `DeleteDirectQueryDataSource`.
pub const DIRECT_QUERY_DATA_SOURCE_QUOTA: usize = 20;

/// The `LimitExceededException` payload for an add past
/// [`DIRECT_QUERY_DATA_SOURCE_QUOTA`] — the modelled refusal for "trying to create more
/// than the allowed number of resources or sub-resources", naming the bound that was
/// reached.
pub fn direct_query_data_source_quota_exceeded() -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "this account already holds the maximum of {DIRECT_QUERY_DATA_SOURCE_QUOTA} \
             direct query data sources"
        )),
    }
}

/// One direct query data source of this account, as this emulator persists it.
///
/// The members are the ones `DirectQueryDataSource` and
/// `GetDirectQueryDataSourceResponse` report between them, so a read projects this
/// record rather than rebuilding one from the request that made it.
#[derive(Debug, Clone)]
pub struct DirectQueryDataSourceRecord {
    /// `DirectQueryDataSource$DataSourceName`. Unique within the account, so the name
    /// IS the key the store holds the record under.
    pub data_source_name: String,
    /// `DirectQueryDataSource$DataSourceType`: the `DirectQueryDataSourceType` union
    /// the call carried, stored as the modelled shape rather than a second mirror of
    /// its members.
    pub data_source_type: DirectQueryDataSourceType,
    /// `DirectQueryDataSource$Description`, absent when the call named none.
    pub description: Option<String>,
    /// `DirectQueryDataSource$OpenSearchArns`, empty when the call named none.
    pub open_search_arns: Vec<String>,
    /// `GetDirectQueryDataSourceResponse$DataSourceAccessPolicy`: the IAM access
    /// policy document the call carried, in the form [`stored_policy_document`] holds
    /// it — the caller's own tokens and values, with the insignificant whitespace
    /// between them dropped — and absent where the call named no document.
    pub data_source_access_policy: Option<String>,
    /// `DirectQueryDataSource$DataSourceArn`: "the unique, system-generated identifier
    /// that represents the data source", minted by
    /// [`direct_query_data_source_arn`] when the source is added and stable for the
    /// life of the record.
    pub arn: String,
    /// `DirectQueryDataSource$TagList`: "a list of tags attached to a direct query
    /// data source", as the add carried them and as the tagging operations read and
    /// write them through
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::tagged_resource`].
    pub tags: TagSet,
}

impl DirectQueryDataSourceRecord {
    /// The collections this data source is associated with as a response reports
    /// them: absent where it is associated with none, because a member the record
    /// has nothing for is one the service leaves out rather than one it answers with
    /// an empty list.
    pub fn reported_open_search_arns(&self) -> Option<Vec<String>> {
        (!self.open_search_arns.is_empty()).then(|| self.open_search_arns.clone())
    }

    /// The tags attached to this data source as a response reports them, on the same
    /// rule as [`Self::reported_open_search_arns`].
    pub fn reported_tag_list(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| tag_list(&self.tags))
    }

    /// This record as the modelled `DirectQueryDataSource` a listing reports, from
    /// the SAME stored record `GetDirectQueryDataSource` answers with — so a data
    /// source's row in the inventory and its own read cannot disagree about its
    /// type, description, collections, identifier or tags.
    ///
    /// `DirectQueryDataSource` is "the configuration details for a data source that
    /// can be directly queried": the members it declares, and no others, so the
    /// access policy only the single read models is not reported here.
    pub fn to_summary(&self) -> DirectQueryDataSource {
        DirectQueryDataSource {
            data_source_name: Some(self.data_source_name.clone()),
            data_source_type: Some(self.data_source_type.clone()),
            description: self.description.clone(),
            open_search_arns: self.reported_open_search_arns(),
            data_source_arn: Some(self.arn.clone()),
            tag_list: self.reported_tag_list(),
        }
    }
}

/// Which member of `DirectQueryDataSourceType` a data source is, as the model names
/// that member — what a refusal about the type names.
pub fn direct_query_data_source_type_name(
    data_source_type: &DirectQueryDataSourceType,
) -> &'static str {
    match data_source_type {
        DirectQueryDataSourceType::CloudWatchLog(_) => "CloudWatchLog",
        DirectQueryDataSourceType::SecurityLake(_) => "SecurityLake",
        DirectQueryDataSourceType::Prometheus(_) => "Prometheus",
    }
}

/// Whether a data source of this type must name the OpenSearch collections it is
/// associated with: `OpenSearchArns` "is required for CloudWatchLogs and SecurityLake
/// datasource types"
/// (`smithy_query(query="operation",operation="AddDirectQueryDataSource")`), and the
/// remaining member of the union asks for none.
pub fn requires_open_search_arns(data_source_type: &DirectQueryDataSourceType) -> bool {
    matches!(
        data_source_type,
        DirectQueryDataSourceType::CloudWatchLog(_) | DirectQueryDataSourceType::SecurityLake(_)
    )
}

/// A direct query data source's ARN:
/// `arn:aws:es:<region>:<account-id>:datasource/<data-source-name>`.
///
/// Built through the same shared helper as [`domain_arn`], so it carries the
/// partition, Region and account id every other ARN this service hands out does, under
/// the `datasource` resource type that names a data source of this service.
pub fn direct_query_data_source_arn(data_source_name: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("datasource/{data_source_name}"),
    )
}

/// The `ValidationException` payload for a name that names no direct query data
/// source of this account.
///
/// Resolving the name a request carries is part of READING that request, so a name
/// that resolves to nothing is a value the caller has to fix rather than a resource
/// the service went looking for. Its wording is the service's own constant for that
/// condition — a fixed sentence every caller that hits it gets, carrying no value of
/// the request, the way [`concurrent_package_operation`] states its condition.
pub fn direct_query_data_source_absent() -> ValidationException {
    ValidationException {
        message: Some("Data source not found".to_string()),
    }
}

/// The `ValidationException` payload for an add naming a direct query data source the
/// account already holds — the mirror of [`data_source_exists`], so one refusal reads
/// like the other.
pub fn direct_query_data_source_exists(data_source_name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "DirectQueryDataSource with name {data_source_name} already exists."
        )),
    }
}

/// The `ValidationException` payload for a data source whose type requires the
/// OpenSearch collections it is associated with ([`requires_open_search_arns`]) and
/// whose request named none. It names the member and the type that asks for it, so the
/// caller can fix the request from the refusal alone.
pub fn open_search_arns_required(data_source_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "OpenSearchArns is required for the {data_source_type} data source type."
        )),
    }
}

/// The `ValidationException` payload for a policy document that is not JSON, naming
/// the member that carried it.
pub fn malformed_policy_document(member: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must be a policy document in valid JSON format."
        )),
    }
}

/// A policy document as this service persists and reports it, or the refusal for one
/// it cannot read: `None` where the request named no document at all.
///
/// The text is first held to its member's modelled `@pattern` over its whole value
/// ([`whole_value_pattern`]), the constraint every input member is settled against at
/// the boundary.
///
/// A policy document "must be in valid JSON format and follow IAM policy syntax", so
/// the text is READ as JSON: one that is not JSON is a malformed request
/// ([`malformed_policy_document`]), a blank one names no policy, and one that IS JSON
/// is held in the service's own rendering of the document it denotes — the tokens the
/// caller wrote, in the order they were written, with the whitespace BETWEEN them (which
/// denotes nothing) dropped. That rendering is [`::service_common::json_document`], the
/// ONE such scan every service that stores an opaque JSON document shares, so no two of
/// them report one document differently. No value is re-rendered from a parse: a number
/// keeps the digits it was written with and a string its exact content, so a document's
/// precision survives a round trip through the store.
pub fn stored_policy_document(
    member: &str,
    document: Option<String>,
) -> Result<Option<String>, ValidationException> {
    let Some(document) = document else {
        return Ok(None);
    };
    // The member's modelled `@pattern` is satisfied only by a value that matches it over
    // its WHOLE length, and the document is held to it BEFORE anything is read out of it:
    // a constraint on an input member is settled at the boundary, so what the text
    // denotes — a document, or no policy at all — is never what decides whether the value
    // was admissible. See [`whole_value_pattern`] for why this member is checked here.
    if let Some(message) = whole_value_pattern::policy_document_refusal(&document) {
        return Err(ValidationException {
            message: Some(message),
        });
    }
    if document.trim().is_empty() {
        return Ok(None);
    }
    ::serde_json::from_str::<::serde_json::Value>(&document)
        .map_err(|_| malformed_policy_document(member))?;
    Ok(Some(::service_common::json_document::minified(&document)))
}

// ---------------------------------------------------------------------------
// VPC endpoint access
//
// An authorization is a principal's access to ONE domain through an interface
// VPC endpoint: `AuthorizeVpcEndpointAccess` grants it, `ListVpcEndpointAccess`
// reports every principal that holds one and `RevokeVpcEndpointAccess` takes it
// away. The set lives on the domain itself ([`Domain::vpc_endpoint_access`]).
// ---------------------------------------------------------------------------

/// `PrincipalType`'s `AWS_ACCOUNT` value: the authorized principal is an Amazon Web
/// Services account, named by its account id.
pub const PRINCIPAL_TYPE_AWS_ACCOUNT: &str = "AWS_ACCOUNT";

/// `PrincipalType`'s `AWS_SERVICE` value — the other member of that same two-value
/// enum: the authorized principal is a service, named by its service principal.
pub const PRINCIPAL_TYPE_AWS_SERVICE: &str = "AWS_SERVICE";

/// How many principals ONE domain's VPC endpoint access set holds before
/// `AuthorizeVpcEndpointAccess` refuses with the modelled `LimitExceededException`.
///
/// Like [`DATA_SOURCE_QUOTA`] this is the emulator's OWN bound on the store — it
/// models neither Service Quotas nor an account's adjustments — and it is what stops
/// a caller looping with fresh principals from growing the state without end: every
/// record it counts is released when `RevokeVpcEndpointAccess` releases the one
/// principal's access, or when `DeleteDomain` releases the domain that holds the set.
/// Because the per-domain set IS what `ListVpcEndpointAccess` answers with, and that
/// set is held on the domain, with [`DOMAIN_QUOTA`] it also bounds the whole
/// authorization store.
pub const VPC_ENDPOINT_ACCESS_QUOTA: usize = 50;

/// The principal a VPC endpoint access call names: what kind of principal it is, and the
/// identity that names it.
#[derive(Debug, Clone)]
pub struct RequestedPrincipal {
    /// One of `PrincipalType`'s values — [`PRINCIPAL_TYPE_AWS_ACCOUNT`] or
    /// [`PRINCIPAL_TYPE_AWS_SERVICE`].
    pub principal_type: &'static str,
    /// `AuthorizedPrincipal$Principal`, and the key a domain's set holds the record
    /// under.
    pub id: String,
}

/// The principal an `Account`/`Service` pair names, or the refusal for a request that
/// names no principal or more than one.
///
/// An authorization is ONE principal's access to the domain and the grant's answer
/// carries exactly one `AuthorizedPrincipal`, whose `PrincipalType` says which of the two
/// kinds it is — so a request names EXACTLY one of them. Both members are optional in the
/// model and nothing in it relates them, so neither combination is refused at the request
/// boundary: it is the operation's own modelled `ValidationException` that refuses them,
/// with 400.
///
/// `AuthorizeVpcEndpointAccess` and `RevokeVpcEndpointAccess` take that same pair and
/// address the same per-domain set with it, so the rule reading it lives here once and
/// the two cannot disagree about which principal a request named.
pub fn requested_principal(
    account: Option<String>,
    service: Option<String>,
) -> Result<RequestedPrincipal, ValidationException> {
    match (account, service) {
        (Some(account), None) => Ok(RequestedPrincipal {
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            id: account,
        }),
        (None, Some(service)) => Ok(RequestedPrincipal {
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            id: service,
        }),
        (None, None) => Err(ValidationException {
            message: Some(
                "either Account or Service is required: it names the principal whose \
                 access to the domain through an interface VPC endpoint this call changes"
                    .to_string(),
            ),
        }),
        (Some(_), Some(_)) => Err(ValidationException {
            message: Some(
                "Account and Service are mutually exclusive: one call changes the access \
                 of one principal"
                    .to_string(),
            ),
        }),
    }
}

/// Where an authorization listing resumes from, or `None` where `next_token` names
/// no place in `ordered`.
///
/// `ListVpcEndpointAccess` reports a domain's WHOLE authorization set in one answer —
/// the set is bounded by [`VPC_ENDPOINT_ACCESS_QUOTA`], so no answer leaves anything
/// unreported — and therefore hands out no continuation token of its own. This is the
/// one place that decides what a token sent to it means: the EMPTY token is the start
/// of the set, as naming none is, and a token naming a principal the set holds is the
/// place the answer starts at. Anything else names no place, which the caller words as
/// the modelled refusal.
pub fn vpc_endpoint_access_start<'a, I>(ordered: I, next_token: Option<&str>) -> Option<usize>
where
    I: IntoIterator<Item = &'a String>,
{
    match next_token.filter(|token| !token.is_empty()) {
        None => Some(0),
        Some(token) => ordered
            .into_iter()
            .position(|principal| principal == token),
    }
}

// ---------------------------------------------------------------------------
// Service-managed VPC endpoints
//
// An endpoint is the proxy a caller in ANOTHER VPC reaches one domain through:
// `CreateVpcEndpoint` mints one, `DescribeVpcEndpoints` reports the ones a
// request names, `ListVpcEndpoints` reports the account's set and
// `DeleteVpcEndpoint` releases one. [`VpcEndpointRecord`] below is the ONE copy
// all of them read and write, plus the two projections the model asks for
// (`VpcEndpoint` and `VpcEndpointSummary`), so no two of them can report the
// same endpoint differently.
//
// The endpoint is held BESIDE the domains rather than on the domain it serves:
// it is addressed by its own id, by callers that name no domain at all
// (`DescribeVpcEndpoints`, `ListVpcEndpoints`), and it names the domain by ARN
// the way the model does.
// ---------------------------------------------------------------------------

/// `VpcEndpointStatus`'s `CREATING` value: the endpoint has been asked for and is
/// being provisioned.
pub const VPC_ENDPOINT_STATUS_CREATING: &str = "CREATING";

/// `VpcEndpointStatus`'s `ACTIVE` value: the endpoint is up and can be connected
/// through.
pub const VPC_ENDPOINT_STATUS_ACTIVE: &str = "ACTIVE";

/// `VpcEndpointStatus`'s `UPDATING` value: a change to the endpoint's subnets or
/// security groups has been asked for and is being applied.
pub const VPC_ENDPOINT_STATUS_UPDATING: &str = "UPDATING";

/// `VpcEndpointStatus`'s `DELETING` value: the endpoint has been released and is
/// being torn down.
pub const VPC_ENDPOINT_STATUS_DELETING: &str = "DELETING";

/// The words the service names a change in flight on a service-managed VPC endpoint
/// by, in the conflict refusal a second writer gets: the lifecycle verb for each
/// transient `VpcEndpointStatus`, lower-case as the refusal states it.
pub const VPC_ENDPOINT_CHANGE_CREATE: &str = "create";
/// The verb for [`VPC_ENDPOINT_STATUS_UPDATING`]. See [`VPC_ENDPOINT_CHANGE_CREATE`].
pub const VPC_ENDPOINT_CHANGE_UPDATE: &str = "update";
/// The verb for [`VPC_ENDPOINT_STATUS_DELETING`]. See [`VPC_ENDPOINT_CHANGE_CREATE`].
pub const VPC_ENDPOINT_CHANGE_DELETE: &str = "delete";

/// `VpcEndpointErrorCode`'s `ENDPOINT_NOT_FOUND` value — the code
/// `DescribeVpcEndpoints` reports IN ITS PAYLOAD for an id that names no endpoint,
/// rather than refusing the whole call: that read reports on a LIST of ids, so one
/// id nothing holds is an error about that id and not about the request.
pub const VPC_ENDPOINT_ERROR_ENDPOINT_NOT_FOUND: &str = "ENDPOINT_NOT_FOUND";

/// How many service-managed VPC endpoints this account holds before
/// `CreateVpcEndpoint` refuses with the modelled `LimitExceededException`.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it models
/// neither Service Quotas nor an account's adjustments — and it is what stops a
/// caller looping from growing the state without end: every record it counts is
/// released by deleting the endpoint. Because the stored set IS what
/// `ListVpcEndpoints` reports, it also bounds that listing.
pub const VPC_ENDPOINT_QUOTA: usize = 50;

/// An endpoint listing's answer: the summaries it carries, or `None` where
/// `next_token` names no place in `ordered`.
///
/// BOTH operations that list endpoints go through this — the account's whole set and
/// one domain's — so they cannot drift on what an answer holds or on what a token
/// sent to them means. `ordered` is the set in the order the caller reports it, which
/// is the store's own key order, and is what makes a token a PLACE in the set rather
/// than an offset a create or a release would have shifted.
///
/// The answer runs to the END of the set: the stored set is bounded by
/// [`VPC_ENDPOINT_QUOTA`], so one answer always reports everything the caller has not
/// been shown, and neither listing has a continuation token to hand out. The EMPTY
/// token therefore means the start of the set, exactly as naming no token does, and
/// `None` — a token naming no endpoint of the set — is left for the caller to word,
/// which is the modelled `ValidationException` both listings refuse it with
/// ([`invalid_next_token`]).
pub fn vpc_endpoint_listing(
    ordered: &[&VpcEndpointRecord],
    next_token: Option<&str>,
) -> Option<Vec<VpcEndpointSummary>> {
    let start = match next_token.filter(|token| !token.is_empty()) {
        None => 0,
        Some(token) => ordered
            .iter()
            .position(|endpoint| endpoint.vpc_endpoint_id == token)?,
    };
    Some(
        ordered
            .iter()
            .skip(start)
            .map(|endpoint| endpoint.to_summary())
            .collect(),
    )
}

/// One service-managed VPC endpoint of this account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct VpcEndpointRecord {
    /// `VpcEndpoint$VpcEndpointId`: unique per endpoint, so it IS the key the store
    /// holds the record under.
    pub vpc_endpoint_id: String,
    /// `VpcEndpoint$VpcEndpointOwner`: "the creator of the endpoint", which is the
    /// account this emulator serves.
    pub owner: String,
    /// `VpcEndpoint$DomainArn`: the domain the endpoint connects to, as the request
    /// named it — and it named a domain this account holds, which is what
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::vpc_endpoint_domain`]
    /// resolved before the record was made.
    pub domain_arn: String,
    /// `VpcEndpoint$VpcOptions`: the subnets and security groups the endpoint was
    /// created with, stored as the modelled shape the read paths report rather than
    /// as a second mirror of its members.
    pub vpc_options: VPCDerivedInfo,
    /// Where the endpoint is in its lifecycle: one of `VpcEndpointStatus`'s values.
    pub status: String,
    /// The status move the clock owes this endpoint, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// `VpcEndpoint$Endpoint`: "the connection endpoint ID for connecting to the
    /// domain".
    pub endpoint: String,
    /// The request's `ClientToken`, absent where it carried none: "unique,
    /// case-sensitive identifier to ensure idempotency of the request", so a repeat
    /// of the same create is answered with THIS record rather than minting a second
    /// endpoint.
    pub client_token: Option<String>,
}

impl VpcEndpointRecord {
    /// Materialise the status move the clock owes this endpoint, if `now` is past its
    /// due instant: an endpoint that was being provisioned becomes one that is up.
    ///
    /// This is the only write a reporting operation makes, and it writes only what was
    /// already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
    }

    /// Whether a change to this endpoint has not landed yet, which is what makes a
    /// second writer's call conflict with the one already in flight.
    ///
    /// Read AFTER [`Self::settle`], so a move that has merely fallen due does not read
    /// as one still in flight. The same shape as [`PackageAssociation::in_flight`],
    /// because it is the same question about a resource this control plane is still
    /// changing.
    pub fn in_flight(&self) -> bool {
        self.owed.is_some()
    }

    /// How the service names the change that has not landed on this endpoint yet, or
    /// `None` where nothing is in flight.
    ///
    /// The endpoint's own status IS that change — it is the status the service leaves an
    /// endpoint in while the change is applied — so the word comes from the status
    /// rather than from a second record of what the last writer asked for, and from the
    /// stored state rather than from the request now being refused: what the refused
    /// caller has to wait for is the change ALREADY under way, whichever call started it.
    pub fn change_in_flight(&self) -> Option<&'static str> {
        if !self.in_flight() {
            return None;
        }
        Some(match self.status.as_str() {
            VPC_ENDPOINT_STATUS_CREATING => VPC_ENDPOINT_CHANGE_CREATE,
            VPC_ENDPOINT_STATUS_UPDATING => VPC_ENDPOINT_CHANGE_UPDATE,
            // The release takes the record out of the store and leaves no move owed on
            // it, so no endpoint this mapping is asked about is being torn down today;
            // the arm is here because the mapping is about the STATUS and is answered
            // the same way whichever status the record carries.
            VPC_ENDPOINT_STATUS_DELETING => VPC_ENDPOINT_CHANGE_DELETE,
            // A move owed from any OTHER status: the three above are every status this
            // store leaves an endpoint in while a change is applied, so this is the
            // surprising case, and a modification of the endpoint is what it reports.
            _ => VPC_ENDPOINT_CHANGE_UPDATE,
        })
    }

    /// Apply the subnets and security groups `options` names to this endpoint and
    /// leave it in the status that change starts.
    ///
    /// `VpcOptions` is "the security groups and/or subnets to add, remove, or modify":
    /// a member the request CARRIES replaces what the endpoint had, and a member it
    /// leaves out names nothing to change, so it keeps the value the endpoint already
    /// carries rather than being cleared.
    ///
    /// The change is not in place the instant the call returns: the endpoint is being
    /// modified, and the clock owes it the status that modification leaves it in once
    /// the span has passed — which is what a second writer arriving meanwhile sees
    /// ([`Self::in_flight`]).
    pub fn begin_update(&mut self, options: VPCOptions, now: SystemTime) {
        if options.subnet_ids.is_some() {
            self.vpc_options.subnet_ids = options.subnet_ids;
        }
        if options.security_group_ids.is_some() {
            self.vpc_options.security_group_ids = options.security_group_ids;
        }
        if options.egress_enabled.is_some() {
            self.vpc_options.egress_enabled = options.egress_enabled;
        }
        self.status = VPC_ENDPOINT_STATUS_UPDATING.to_string();
        self.owed = Some(OwedStatus {
            status: VPC_ENDPOINT_STATUS_ACTIVE.to_string(),
            message: None,
            due: now + PROVISIONING,
        });
    }

    /// This record as `VpcEndpoint` — the shape `CreateVpcEndpoint` answers with and
    /// `DescribeVpcEndpoints` reports, so a create's answer and a later read of the
    /// endpoint cannot disagree.
    pub fn to_endpoint(&self) -> VpcEndpoint {
        VpcEndpoint {
            vpc_endpoint_id: Some(self.vpc_endpoint_id.clone()),
            vpc_endpoint_owner: Some(self.owner.clone()),
            domain_arn: Some(self.domain_arn.clone()),
            vpc_options: Some(self.vpc_options.clone()),
            status: Some(self.status.clone()),
            endpoint: Some(self.endpoint.clone()),
        }
    }

    /// This record as `VpcEndpointSummary` — the shape `ListVpcEndpoints` pages
    /// through and `DeleteVpcEndpoint` answers with: the same record as
    /// [`Self::to_endpoint`] without the subnets and the connection endpoint, which
    /// the model's summary declares no member for.
    pub fn to_summary(&self) -> VpcEndpointSummary {
        VpcEndpointSummary {
            vpc_endpoint_id: Some(self.vpc_endpoint_id.clone()),
            vpc_endpoint_owner: Some(self.owner.clone()),
            domain_arn: Some(self.domain_arn.clone()),
            status: Some(self.status.clone()),
        }
    }
}

/// The `ValidationException` payload for a `CreateVpcEndpoint` whose `DomainArn`
/// names no domain this account holds, or names one no endpoint can be created for.
///
/// The service states these as ONE refusal, and deliberately: which of them holds is
/// something the caller may not be entitled to learn, so the sentence names all the
/// reasons rather than the one that applied. It carries no value of the request for
/// the same reason.
pub fn vpc_endpoint_domain_unavailable() -> ValidationException {
    ValidationException {
        message: Some(
            "Either the domain doesn't exist, it doesn't support creation of VPC \
             endpoints, or you don't have permission to create a VPC endpoint for the \
             domain."
                .to_string(),
        ),
    }
}

/// The sentence `DescribeVpcEndpoints` states IN ITS PAYLOAD for an id that names no
/// service-managed VPC endpoint: fixed text carrying no value of the request, because
/// the entry beside it already names the id it is about.
pub const ENDPOINT_DOES_NOT_EXIST: &str = "Endpoint does not exist";

/// The sentence a call that ADDRESSES one endpoint refuses a missing id with.
///
/// Worded differently from [`ENDPOINT_DOES_NOT_EXIST`], and deliberately: a caller
/// addressing an endpoint may not be entitled to learn which of the two reasons holds,
/// so the refusal names both rather than the one that applied — the same reticence
/// [`vpc_endpoint_domain_unavailable`] shows about a domain.
pub const ENDPOINT_INACCESSIBLE: &str =
    "The VPC endpoint doesn't exist, or you don't have access to it.";

/// The `ResourceNotFoundException` payload for an id that names no endpoint of this
/// account — the endpoint mirror of [`domain_not_found`].
///
/// This is what a call ADDRESSING one endpoint (releasing it) answers; a read that
/// names a LIST of ids reports the same condition per id in its payload instead
/// ([`vpc_endpoint_missing`]).
pub fn vpc_endpoint_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(ENDPOINT_INACCESSIBLE.to_string()),
    }
}

/// The `ConflictException` payload for a call that would change a service-managed VPC
/// endpoint whose last change has not landed yet.
///
/// The service applies one change to an endpoint at a time, so a writer that arrives
/// while another is still in flight is refused rather than overtaking it — the same
/// rule [`concurrent_package_operation`] states for a domain's package associations.
/// The sentence states the change ALREADY in progress on the endpoint
/// ([`VpcEndpointRecord::change_in_flight`]) and carries no value of the request: what
/// the refused caller has to wait for is that change, and the endpoint is the one it
/// already named.
pub fn vpc_endpoint_being_modified(change_in_flight: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "A {change_in_flight} operation is already in progress for this VPC endpoint."
        )),
    }
}

/// The `ValidationException` payload for a read that names more endpoint ids than this
/// account can hold endpoints: the answer carries one entry per id the request names,
/// so the request would otherwise choose the size of the response.
pub fn too_many_vpc_endpoint_ids(named: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "VpcEndpointIds names {named} endpoints; at most {VPC_ENDPOINT_QUOTA} can be \
             described in one call."
        )),
    }
}

/// The `VpcEndpointError` `DescribeVpcEndpoints` reports for one id that names no
/// endpoint: the id it was asked about, the modelled code for that condition and the
/// service's own sentence for it.
pub fn vpc_endpoint_missing(vpc_endpoint_id: &str) -> VpcEndpointError {
    VpcEndpointError {
        vpc_endpoint_id: Some(vpc_endpoint_id.to_string()),
        error_code: Some(VPC_ENDPOINT_ERROR_ENDPOINT_NOT_FOUND.to_string()),
        error_message: Some(ENDPOINT_DOES_NOT_EXIST.to_string()),
    }
}

// ---------------------------------------------------------------------------
// OpenSearch UI applications
//
// An application is the OpenSearch user interface over an account's data, and
// an ACCOUNT resource of this control plane rather than a sub-resource of a
// domain: it is addressed by its own id and names no domain at all.
// `CreateApplication` mints one and `GetApplication` reports it, both from the
// ONE [`Application`] record below, so the answer a create gives and the answer
// a later read gives cannot disagree.
// ---------------------------------------------------------------------------

/// `ApplicationStatus`'s `CREATING` value: the application has been asked for and is
/// being provisioned.
pub const APPLICATION_STATUS_CREATING: &str = "CREATING";

/// `ApplicationStatus`'s `ACTIVE` value: the application is up and can be used.
pub const APPLICATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `ApplicationStatus`'s `DELETING` value: the application's removal has been asked
/// for and is being carried out.
pub const APPLICATION_STATUS_DELETING: &str = "DELETING";

/// How many OpenSearch UI applications this account holds before
/// `CreateApplication` refuses to mint another.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it models
/// neither Service Quotas nor an account's adjustments — and it is what stops a caller
/// looping with fresh names from growing the state without end: every record it counts
/// is released when the delete that asked for its removal completes
/// ([`Application::begin_deletion`]).
pub const APPLICATION_QUOTA: usize = 100;

/// One OpenSearch UI application of this account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Application {
    /// The application's `Id`: unique per application, so it IS the key the store
    /// holds the record under, and the value every read path addresses it by.
    pub id: String,
    /// `ApplicationName`: "the unique name of the OpenSearch application. Names must
    /// be unique within an Amazon Web Services Region for each account", which is
    /// what [`ComAmazonawsOpensearchAmazonopensearchserviceState::application_named`]
    /// resolves a create against.
    pub name: String,
    /// The application's `ARN`, built by [`application_arn`] from the id the store
    /// minted.
    pub arn: String,
    /// "The endpoint URL of an OpenSearch application" ([`application_endpoint`]).
    pub endpoint: String,
    /// Where the application is in its lifecycle: one of `ApplicationStatus`'s
    /// values.
    pub status: String,
    /// The status move the clock owes this application, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// The `DataSources` the application was created with, stored as the modelled
    /// shape every read path reports rather than as a second mirror of its members.
    ///
    /// The modelled shape is imported as `LinkedDataSource` because this module also
    /// holds a [`DataSource`] of its own — a domain's direct-query data source, a
    /// different resource that happens to share the model's name.
    pub data_sources: Vec<LinkedDataSource>,
    /// This application's data source attachments, keyed by the `dataSourceArn`
    /// they link it to — one attachment per data source, so the ARN IS the key,
    /// and the value `DetachDataSource` and `DescribeDataSourceAttachment`
    /// address a record by. Iterating a `BTreeMap` is order-stable, which is what
    /// lets `ListDataSourceAttachments` report the same order twice and page
    /// through the set without repeating or skipping a record. Bounded by
    /// [`ATTACHMENT_QUOTA`].
    pub attachments: BTreeMap<String, DataSourceAttachment>,
    /// The `AppConfigs` the application was created with, stored as the modelled
    /// shape every read path reports.
    pub app_configs: Vec<AppConfig>,
    /// The IAM Identity Center settings the application carries, absent where the
    /// create asked for none ([`stored_iam_identity_center_options`]).
    pub iam_identity_center_options: Option<IamIdentityCenterOptions>,
    /// `KmsKeyArn`: the customer-managed key the application's data is encrypted
    /// with. Absent where the create named none — "if omitted, the application uses
    /// an AWS-managed key", and that key's ARN is KMS's to mint rather than this
    /// emulator's to invent.
    pub kms_key_arn: Option<String>,
    /// The application's tags, keyed by tag key the way every taggable resource of
    /// this store holds them, so the tagging operations read and write THIS set.
    pub tags: TagSet,
    /// When the application was created.
    pub created_at: SystemTime,
    /// When the application was last changed — the creation instant for one no call
    /// has changed since.
    pub last_updated_at: SystemTime,
    /// The instant this application's record is gone, set by the delete that asked for
    /// its removal and absent on one nothing has asked to remove.
    ///
    /// A delete does not take the record away where it stands: the application rests
    /// in its deleting status, and what the clock owes it is its removal — the same
    /// shape as the status moves [`OwedStatus`] carries, for a move whose outcome is
    /// that the account holds no such application at all.
    pub removal_due: Option<SystemTime>,
    /// The request's `ClientToken`, absent where it carried none: "unique,
    /// case-sensitive identifier to ensure idempotency of the request", so a repeat
    /// of the same create is answered with THIS record rather than minting a second
    /// application.
    pub client_token: Option<String>,
}

impl Application {
    /// Materialise the status move the clock owes this application, if `now` is past
    /// its due instant: one that was being provisioned becomes one that is up.
    ///
    /// This is the only write a reporting operation makes, and it writes only what was
    /// already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
    }

    /// Start this application's deletion, as the call that asked for it leaves the
    /// record — the application mirror of [`Package::begin_deletion`].
    ///
    /// The application is not gone the instant that call returns: it is being deleted,
    /// so it rests in [`APPLICATION_STATUS_DELETING`] and the clock owes the record's
    /// removal one [`PROVISIONING`] span later, which
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::settle_applications`]
    /// performs when it falls due. Whatever status move was outstanding is dropped: an
    /// application on its way out is on its way to no other status.
    ///
    /// Idempotent — an application whose removal the clock already owes keeps the
    /// instant it was owed at, so repeating the call cannot push the removal back.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.owed = None;
        self.status = APPLICATION_STATUS_DELETING.to_string();
        self.last_updated_at = now;
        self.removal_due = Some(now + PROVISIONING);
    }

    /// Whether the removal the clock owed this application has fallen due by `now`, so
    /// the account no longer holds it.
    pub fn removal_landed(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// Apply the settings an update asked to change to this application and record
    /// when it changed.
    ///
    /// A member the request CARRIES replaces what the application held, and a member
    /// it leaves out names nothing to change, so the record keeps the value it already
    /// carries rather than having it cleared — the same rule
    /// [`VpcEndpoint::begin_update`] applies to an endpoint's VPC options. An
    /// explicitly empty list IS a value the request carries, so it leaves the
    /// application holding none.
    ///
    /// The application's lifecycle status is not this call's to move: what an update
    /// changes is the configuration the record carries, and the moves the clock owes
    /// this application stay exactly as they were ([`Self::settle`]).
    pub fn apply_update(
        &mut self,
        data_sources: Option<Vec<LinkedDataSource>>,
        app_configs: Option<Vec<AppConfig>>,
        iam_identity_center_options: Option<IamIdentityCenterOptionsInput>,
        now: SystemTime,
    ) {
        if let Some(data_sources) = data_sources {
            self.data_sources = data_sources;
        }
        if let Some(app_configs) = app_configs {
            self.app_configs = app_configs;
        }
        if let Some(asked) = iam_identity_center_options {
            self.iam_identity_center_options = Some(updated_iam_identity_center_options(
                self.iam_identity_center_options.take(),
                asked,
            ));
        }
        self.last_updated_at = now;
    }

    /// The application's data sources as a response reports them: absent where it
    /// holds none, because a member the application has nothing for is one the
    /// service leaves out rather than one it answers with an empty list.
    pub fn reported_data_sources(&self) -> Option<Vec<LinkedDataSource>> {
        (!self.data_sources.is_empty()).then(|| self.data_sources.clone())
    }

    /// The application's configuration settings as a response reports them, on the
    /// same rule as [`Self::reported_data_sources`].
    pub fn reported_app_configs(&self) -> Option<Vec<AppConfig>> {
        (!self.app_configs.is_empty()).then(|| self.app_configs.clone())
    }

    /// The application's tags as a response reports them, on the same rule as
    /// [`Self::reported_data_sources`].
    pub fn reported_tag_list(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| tag_list(&self.tags))
    }

    /// This application as a listing reports it, from the SAME stored record a single
    /// read answers with — so an application's row in a listing and its own
    /// `GetApplication` cannot disagree about where it is in its lifecycle or when it
    /// was last changed.
    ///
    /// `ApplicationSummary` is "basic details of an OpenSearch application": the
    /// members it declares, and no others, so the configuration a read reports in full
    /// is not duplicated here.
    pub fn to_summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            endpoint: Some(self.endpoint.clone()),
            status: Some(self.status.clone()),
            created_at: Some(datetime(self.created_at)),
            last_updated_at: Some(datetime(self.last_updated_at)),
        }
    }

    /// The instant this application becomes usable, or `None` when it already is.
    ///
    /// An application that is not up yet is one being provisioned, and such a record
    /// carries the status move the clock owes it ([`Self::owed`]), whose due instant is
    /// when it becomes active.
    pub fn active_at(&self) -> Option<SystemTime> {
        if self.status == APPLICATION_STATUS_ACTIVE {
            return None;
        }
        self.owed.as_ref().map(|owed| owed.due)
    }

    /// The attachment of `data_source_arn`, if this application holds one.
    ///
    /// Reads ONLY.
    pub fn attachment(&self, data_source_arn: &str) -> Option<&DataSourceAttachment> {
        self.attachments.get(data_source_arn)
    }

    /// The attachment an attach carrying `client_token` already minted on this
    /// application, if it holds one: "if you retry a request with the same client token
    /// and the same parameters, the retry succeeds without performing any further
    /// actions", so a repeat is answered with the record the first call made.
    ///
    /// Reads ONLY.
    pub fn attachment_of_client_token(&self, client_token: &str) -> Option<&DataSourceAttachment> {
        self.attachments
            .values()
            .find(|attachment| attachment.client_token.as_deref() == Some(client_token))
    }

    /// This application's data source attachments in the order a listing reports them:
    /// the order they were attached in, oldest first.
    ///
    /// Two attachments made at one instant are ordered by the data source they attach,
    /// which is unique per attachment — so the order is TOTAL, and the same set reports
    /// the same order twice however the records are held. That is what lets a page token
    /// name a place in the set.
    pub fn attachments_in_creation_order(&self) -> Vec<&DataSourceAttachment> {
        let mut attachments: Vec<&DataSourceAttachment> = self.attachments.values().collect();
        attachments.sort_by(|one, other| {
            one.created_at
                .cmp(&other.created_at)
                .then_with(|| one.data_source_arn.cmp(&other.data_source_arn))
        });
        attachments
    }
}

/// An application's ARN: `arn:aws:es:<region>:<account-id>:application/<id>`.
///
/// The same namespace as [`domain_arn`] — one service, one `arnNamespace` — with the
/// resource type the application is addressed as.
pub fn application_arn(id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("application/{id}"))
}

/// The URL an OpenSearch UI application is reached on: the application's own id, the
/// Region it was created in, then the user-interface host of this service.
///
/// The model types the member as a plain string, so its SHAPE is the service's to
/// pick: it is derived from the id rather than the name, so two applications never
/// share an endpoint and renaming one cannot move it.
pub fn application_endpoint(id: &str, region: &str) -> String {
    format!("https://{id}.{region}.opensearch.amazonaws.com")
}

/// The `ConflictException` payload for a create whose `name` is already an
/// application's of this account.
///
/// "Names must be unique within an Amazon Web Services Region for each account", so a
/// second create of the same name asks for a second application under one name and is
/// refused rather than minting one.
pub fn application_name_taken(name: &str) -> ConflictException {
    ConflictException {
        message: Some(format!("Application with name {name} already exists.")),
    }
}

/// What this service states about an id that names no application of this account.
///
/// The statement names no id: the request carries the one it could not resolve, so the
/// answer states only that the account holds no such application.
pub const APPLICATION_DOES_NOT_EXIST: &str = "Application does not exist";

/// The `ResourceNotFoundException` payload for an id that names no application of
/// this account — the application mirror of [`domain_not_found`].
pub fn application_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(APPLICATION_DOES_NOT_EXIST.to_string()),
    }
}

/// The `ValidationException` payload for a create made while the account already holds
/// [`APPLICATION_QUOTA`] applications.
///
/// `CreateApplication` models no `LimitExceededException`, so the bound on the store is
/// reported through the client-input error it does model.
pub fn application_quota_exceeded() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "This account already holds the maximum of {APPLICATION_QUOTA} applications."
        )),
    }
}

// ---------------------------------------------------------------------------
// Data source attachments
//
// An attachment is a sub-resource of an application: `AttachDataSource` mints
// one, `DescribeDataSourceAttachment` and `ListDataSourceAttachments` report it
// and `DetachDataSource` removes it, all from the ONE [`DataSourceAttachment`]
// record below, held on the application the data source is attached to — so no
// two of those operations can report one attachment differently.
//
// It is a record of its own rather than an entry in the application's
// `DataSources` list: that list is what a create named and what every
// application read reports, while an attachment carries an identifier of its
// own and a status the clock moves.
// ---------------------------------------------------------------------------

/// `DataSourceAttachmentStatus`'s `PENDING` value: the attachment has been asked
/// for and is waiting for the application and the data source to become active.
pub const ATTACHMENT_STATUS_PENDING: &str = "PENDING";

/// `DataSourceAttachmentStatus`'s `ATTACHED` value: the data source is attached.
pub const ATTACHMENT_STATUS_ATTACHED: &str = "ATTACHED";

/// `DataSourceAttachmentStatus`'s `FAILED` value: the attachment could not be
/// completed.
pub const ATTACHMENT_STATUS_FAILED: &str = "FAILED";

/// How many data source attachments one application holds before an attach is
/// refused rather than adding another record.
///
/// Like [`APPLICATION_QUOTA`] this is the emulator's OWN bound on the store — the
/// model publishes none — and it is what stops a caller looping with fresh data
/// sources from growing one application's state without end; every record it
/// counts is released when the data source is detached.
pub const ATTACHMENT_QUOTA: usize = 25;

/// The page `ListDataSourceAttachments` answers with when a request names no page
/// size: "the maximum number of results to return per page. The default is 50"
/// (`ListDataSourceAttachmentsRequest$maxResults`).
pub const ATTACHMENT_PAGE: usize = 50;

/// One data source attachment of an application, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct DataSourceAttachment {
    /// "The unique identifier assigned to the data source attachment", minted by
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_attachment_id`].
    pub attachment_id: String,
    /// The ARN of the data source this attachment links the application to —
    /// unique per attachment of one application, so it IS the key the
    /// application holds the record under, and the value `DetachDataSource` and
    /// `DescribeDataSourceAttachment` address it by.
    pub data_source_arn: String,
    /// Where the attachment stands: one of `DataSourceAttachmentStatus`'s values.
    pub status: String,
    /// The status move the clock owes this attachment, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// When the attachment was asked for.
    pub created_at: SystemTime,
    /// The request's `ClientToken`, absent where it carried none: "if you retry a
    /// request with the same client token and the same parameters, the retry
    /// succeeds without performing any further actions", so a repeat is answered
    /// with THIS record rather than minting a second attachment.
    pub client_token: Option<String>,
}

impl DataSourceAttachment {
    /// Materialise the status move the clock owes this attachment, if `now` is past
    /// its due instant: a pending attachment "completes automatically once both
    /// become active", and one whose data source the account no longer holds can
    /// never complete, which is the non-retryable outcome `FAILED` reports.
    ///
    /// `data_sources` are the ARNs the account still holds a data source under.
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime, data_sources: &BTreeSet<String>) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = if data_sources.contains(&self.data_source_arn) {
            owed.status
        } else {
            ATTACHMENT_STATUS_FAILED.to_string()
        };
    }

    /// This attachment as `DataSourceAttachmentSummary` — what
    /// `ListDataSourceAttachments` reports it as.
    pub fn to_summary(&self) -> DataSourceAttachmentSummary {
        DataSourceAttachmentSummary {
            attachment_id: Some(self.attachment_id.clone()),
            data_source_arn: Some(self.data_source_arn.clone()),
            status: Some(self.status.clone()),
        }
    }
}

/// Where an attachment stands the instant it is asked for, and the status move the
/// clock then owes it.
///
/// "If both the application and the data source are active, the attachment completes
/// immediately with a status of ATTACHED. Otherwise, the operation returns PENDING and
/// completes the attachment automatically once both become active" — so `active_at` is
/// the LATEST instant either of the two becomes active, and `None` when both already
/// are.
pub fn attachment_outcome(active_at: Option<SystemTime>) -> (String, Option<OwedStatus>) {
    match active_at {
        None => (ATTACHMENT_STATUS_ATTACHED.to_string(), None),
        Some(due) => (
            ATTACHMENT_STATUS_PENDING.to_string(),
            Some(OwedStatus {
                status: ATTACHMENT_STATUS_ATTACHED.to_string(),
                message: None,
                due,
            }),
        ),
    }
}

/// AWS Glue's own `arnNamespace`: the Data Catalog an application reaches its
/// direct-query data through is a Glue resource, so the ARN that names it carries
/// Glue's namespace rather than this service's.
const GLUE_ARN_NAMESPACE: &str = "glue";

/// The resource field of the ARN that names an account's AWS Glue Data Catalog,
/// `arn:<partition>:glue:<region>:<account-id>:catalog`.
const GLUE_DATA_CATALOG_RESOURCE: &str = "catalog";

/// Whether `arn` names THIS account's AWS Glue Data Catalog.
///
/// An account holds exactly one Data Catalog per Region and no call creates or deletes
/// it, so an ARN of that form names a resource the account always holds — which is why
/// resolving it consults no stored record. A catalog ARN carrying another account's id
/// names a catalog this account does not hold, and one carrying no Region names no
/// catalog at all.
pub fn is_account_glue_data_catalog_arn(arn: &str) -> bool {
    ::service_common::arn::parse(arn).is_some_and(|parsed| {
        parsed.vendor == GLUE_ARN_NAMESPACE
            && parsed.resource == GLUE_DATA_CATALOG_RESOURCE
            && !parsed.region.is_empty()
            && parsed.account == ::engine::emulator_account_id()
    })
}

/// What this account holds under the ARN an attachment names its data source by, and
/// when that data source becomes active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceActivation {
    /// The account holds the data source and it is active already, so an attachment of
    /// it completes at once.
    Active,
    /// The account holds it and the clock owes it activation at this instant, so an
    /// attachment of it stays pending until then.
    ActiveAt(SystemTime),
    /// The account holds no data source under the ARN, so an attachment of it has
    /// nothing that becomes active.
    Absent,
}

/// The `ResourceNotFoundException` payload for an application and data source pair this
/// account holds no attachment record for.
pub fn attachment_not_found(id: &str, data_source_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Data source {data_source_arn} is not attached to application {id}."
        )),
    }
}

/// The `ConflictException` payload for a detach of a data source whose attachment has
/// not landed yet: "throws a ConflictException if the specified data source has a
/// PENDING attachment".
pub fn attachment_pending(id: &str, data_source_arn: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Data source {data_source_arn} has a {ATTACHMENT_STATUS_PENDING} attachment to \
             application {id}. Wait for the attachment to complete before detaching the data \
             source."
        )),
    }
}

/// The `ConflictException` payload for a detach made while the application is not up:
/// "the application must be in the ACTIVE state".
pub fn application_not_active(id: &str, status: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Application {id} is {status}. The application must be \
             {APPLICATION_STATUS_ACTIVE} to detach a data source."
        )),
    }
}

/// The `ConflictException` payload for an attach whose `clientToken` this application
/// already holds an attachment under, for a DIFFERENT data source.
///
/// "If you retry a request with the same client token and the same parameters, the retry
/// succeeds without performing any further actions": what the token identifies is the
/// attach of ONE data source, so a token already identifying another data source's
/// attach is one token for two attaches, which cannot both be the one it identifies.
pub fn attachment_client_token_taken(
    client_token: &str,
    data_source_arn: &str,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "ClientToken {client_token} already identifies the attachment of data source \
             {data_source_arn}."
        )),
    }
}

/// The `ValidationException` payload for an attach naming both an existing workspace and
/// one to create: `workspaceId` is "mutually exclusive with workspaceConfiguration", so
/// a request carrying both asks for two different workspaces at once.
pub fn workspace_selection_conflict() -> ValidationException {
    ValidationException {
        message: Some(
            "workspaceId and workspaceConfiguration are mutually exclusive: name the workspace \
             to update or the workspace to create, not both."
                .to_string(),
        ),
    }
}

/// The `ValidationException` payload for an attach made while the application already
/// holds [`ATTACHMENT_QUOTA`] attachments.
///
/// `AttachDataSource` models no `LimitExceededException`, so the bound on the store is
/// reported through the client-input error it does model.
pub fn attachment_quota_exceeded(id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Application {id} already holds the maximum of {ATTACHMENT_QUOTA} data source \
             attachments."
        )),
    }
}

// ---------------------------------------------------------------------------
// Saved-object migrations
//
// A migration is the job that moves an account's saved objects (dashboards,
// visualizations, index patterns, searches) out of a data source and into an
// application's workspace: `StartMigration` mints one and `GetMigration`
// reports its progress, both from the ONE [`Migration`] record below — so the
// job a start answered with and the job a later read reports cannot disagree.
//
// It is an ACCOUNT resource keyed by its own `migrationId` rather than a record
// held on the application it migrates into: `GetMigration` addresses a job by
// that id alone and names no application, so the id has to resolve on its own.
// ---------------------------------------------------------------------------

/// A migration job's `PENDING` status: the job has been accepted and its export has
/// not begun — "the status is PENDING when a migration is first created"
/// (`StartMigrationResponse$status`).
pub const MIGRATION_STATUS_PENDING: &str = "PENDING";

/// A migration job's `IN_PROGRESS` status: the export has run and the objects it
/// selected are being imported into the target workspace.
pub const MIGRATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// A migration job's `SUCCEEDED` status: the selected objects are in the workspace.
pub const MIGRATION_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// A migration job's `FAILED` status: the job stopped without importing its objects.
pub const MIGRATION_STATUS_FAILED: &str = "FAILED";

/// Every status a migration job reports, in lifecycle order.
///
/// The model types `status` as a plain string rather than an enum, so the published
/// list is its documentation's: "valid values are PENDING, IN_PROGRESS, SUCCEEDED, and
/// FAILED" (`smithy_query(query="doc",shape="GetMigrationResponse",member="status")`).
pub const MIGRATION_STATUS_VALUES: [&str; 4] = [
    MIGRATION_STATUS_PENDING,
    MIGRATION_STATUS_IN_PROGRESS,
    MIGRATION_STATUS_SUCCEEDED,
    MIGRATION_STATUS_FAILED,
];

/// The conflict-resolution strategies a migration may be started with.
///
/// `conflictResolution` is typed as a plain string, so the published set is its
/// documentation's: "valid values are CREATE_NEW_COPIES, which creates new objects with
/// unique IDs, and overwrite, which replaces existing objects"
/// (`smithy_query(query="doc",shape="MigrationOptions",member="conflictResolution")`) —
/// spelled there exactly as below.
pub const CONFLICT_RESOLUTION_VALUES: [&str; 2] = ["CREATE_NEW_COPIES", "overwrite"];

/// How many migration jobs ONE application holds before a start is refused rather than
/// minting another.
///
/// Like [`ATTACHMENT_QUOTA`] this is the emulator's OWN bound on the store — the model
/// publishes none — and it is what stops a caller looping with fresh jobs from growing
/// the state without end. It bounds the WHOLE set too, because a start resolves its
/// `applicationId` against the applications this account holds and those are bounded by
/// [`APPLICATION_QUOTA`]: no request can partition the store past that product.
pub const MIGRATION_QUOTA: usize = 100;

/// One saved-object migration job of this account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Migration {
    /// "The unique identifier of the migration job", minted by
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_migration_id`] — so
    /// it IS the key the store holds the record under, and the value `GetMigration`
    /// addresses it by.
    pub migration_id: String,
    /// The application the job migrates saved objects INTO, by its `Id`.
    pub application_id: String,
    /// Where the objects come from, stored as the modelled shape every read path
    /// reports rather than as a second mirror of its one member.
    pub source: MigrationSource,
    /// The workspace the objects go into, as the start named it: an existing one by
    /// `workspaceId`, or one the job was asked to create.
    pub workspace: MigrationWorkspace,
    /// Which saved objects the job exports, absent where the start named no filter —
    /// in which case its scope is every saved object the source holds.
    pub export_options: Option<ExportOptions>,
    /// How the import treats an object the workspace already holds, absent where the
    /// start named no strategy.
    pub conflict_resolution: Option<String>,
    /// Where the job stands: one of [`MIGRATION_STATUS_VALUES`].
    pub status: String,
    /// The status move the clock owes this job, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// "The number of saved objects exported from the source data source": zero until
    /// the export step has run.
    pub exported_count: i32,
    /// "The number of saved objects successfully imported into the target workspace":
    /// zero until the import step has run.
    pub imported_count: i32,
    /// When the job was created.
    pub created_at: SystemTime,
    /// When the job last changed — the creation instant for one still in the status it
    /// was created in, and the instant a status move FELL DUE for one the clock has
    /// moved since, rather than whenever a caller happened to read it.
    pub updated_at: SystemTime,
    /// The request's `ClientToken`, absent where it carried none: "if this token matches
    /// a previous request, Amazon OpenSearch Service ignores the request but does not
    /// return an error", so a repeat is answered with THIS record rather than starting a
    /// second job.
    pub client_token: Option<String>,
}

impl Migration {
    /// Materialise every status move the clock owes this job whose due instant has
    /// passed: a `PENDING` job's export runs, and the `IN_PROGRESS` import that follows
    /// it completes.
    ///
    /// The chain is walked from the STORED due instants — the next step falls due a span
    /// after the previous one did, not a span after somebody read the record — so the
    /// status a job is in at `now` is the same whoever calls and whenever. This is the
    /// only write a reporting operation makes, and it writes only what was already owed
    /// at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        while self.owed.as_ref().is_some_and(|owed| now >= owed.due) {
            let Some(owed) = self.owed.take() else { break };
            self.status = owed.status;
            self.updated_at = owed.due;
            self.owed = self.next_step(owed.due);
            if self.status == MIGRATION_STATUS_IN_PROGRESS {
                // The export step has run, so the objects the job selected are out of
                // the source and waiting on the import that follows.
                self.exported_count = self.selected_count();
            }
            if self.status == MIGRATION_STATUS_SUCCEEDED {
                // The import step has run: a job that succeeded left every object it
                // exported in the target workspace.
                self.imported_count = self.exported_count;
            }
        }
    }

    /// The step the clock owes this job once it has reached `status` at `reached`, or
    /// `None` for a job in a status nothing follows.
    ///
    /// The ONE statement of the lifecycle's order, so the step a start owes and the step
    /// a settle owes cannot disagree.
    pub fn next_step(&self, reached: SystemTime) -> Option<OwedStatus> {
        let next = match self.status.as_str() {
            MIGRATION_STATUS_PENDING => MIGRATION_STATUS_IN_PROGRESS,
            MIGRATION_STATUS_IN_PROGRESS => MIGRATION_STATUS_SUCCEEDED,
            _ => return None,
        };
        Some(OwedStatus {
            status: next.to_string(),
            message: None,
            due: reached + PROVISIONING,
        })
    }

    /// How many saved objects this job's export selects.
    ///
    /// `exportOptions.objects` is "a list of specific saved objects to include in the
    /// migration, identified by type and ID", so a job filtered to a list of objects
    /// exports exactly the DISTINCT objects it names. A job that names none has every
    /// saved object of the source in scope, and this control plane holds no record of
    /// the saved objects a data source carries, so there is nothing for it to count.
    pub fn selected_count(&self) -> i32 {
        let objects = self
            .export_options
            .as_ref()
            .and_then(|options| options.objects.as_ref());
        let distinct: BTreeSet<(&str, &str)> = objects
            .into_iter()
            .flatten()
            .map(|object| (object.r#type.as_str(), object.id.as_str()))
            .collect();
        i32::try_from(distinct.len()).unwrap_or(i32::MAX)
    }

    /// This job as a listing reports it, from the SAME stored record a single read
    /// answers with — so a job's row in a listing and its own read cannot disagree
    /// about the status or the progress it is at.
    pub fn to_summary(&self) -> MigrationSummary {
        MigrationSummary {
            migration_id: Some(self.migration_id.clone()),
            status: Some(self.status.clone()),
            application_id: Some(self.application_id.clone()),
            source: Some(self.source.clone()),
            exported_count: self.exported_count,
            imported_count: self.imported_count,
            // "Error details if the migration failed or completed with errors": no path
            // of this control plane fails a job, so there is nothing to report here.
            error: None,
            created_at: Some(datetime(self.created_at)),
            updated_at: Some(datetime(self.updated_at)),
        }
    }
}

/// The `ResourceNotFoundException` payload for an id that addresses no migration job of
/// this account — the migration mirror of [`application_not_found`].
pub fn migration_not_found(migration_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Migration not found: {migration_id}")),
    }
}

/// The `ConflictException` payload for a start whose `clientToken` this account already
/// holds a migration under, for a DIFFERENT application.
///
/// "If this token matches a previous request, Amazon OpenSearch Service ignores the
/// request": what the token identifies is the start of ONE job, so a token already
/// identifying a job of another application is one token for two starts, which cannot
/// both be the one it identifies.
pub fn migration_client_token_taken(client_token: &str, application_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "ClientToken {client_token} already identifies the migration started for \
             application {application_id}."
        )),
    }
}

/// The `ValidationException` payload for a start made while the application already
/// holds [`MIGRATION_QUOTA`] migration jobs.
///
/// `StartMigration` models no `LimitExceededException`, so the bound on the store is
/// reported through the client-input error it does model.
pub fn migration_quota_exceeded(application_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Application {application_id} already holds the maximum of {MIGRATION_QUOTA} \
             migrations."
        )),
    }
}

/// Why `migrationOptions.workspace` names no target the import could write to, or
/// `None` for one that does — the ONE statement of what a migration's target must be.
///
/// "Specify either this parameter or `createWorkspace`"
/// (`MigrationWorkspace$workspaceId`) and "required when `createWorkspace` is true"
/// (`MigrationWorkspace$name`): a request naming both an existing workspace and a new
/// one asks for two targets, one naming neither asks for none, and one asking for a new
/// workspace without a name leaves it unnamed — none of which is a choice this service
/// makes for the caller.
pub fn migration_workspace_refusal(workspace: &MigrationWorkspace) -> Option<ValidationException> {
    let creating = workspace.create_workspace.unwrap_or_default();
    let named = workspace
        .name
        .as_deref()
        .is_some_and(|name| !name.is_empty());
    let message = match (workspace.workspace_id.is_some(), creating) {
        (true, true) => {
            "migrationOptions.workspace: workspaceId and createWorkspace are mutually \
             exclusive: name the workspace to migrate into or ask for a new one, not both."
        }
        (false, false) => {
            "migrationOptions.workspace: name the workspace to migrate into with workspaceId, \
             or ask for a new one with createWorkspace."
        }
        (false, true) if !named => {
            "migrationOptions.workspace: name is required when createWorkspace is true."
        }
        _ => return None,
    };
    Some(ValidationException {
        message: Some(message.to_string()),
    })
}

/// The `ValidationException` payload for a `conflictResolution` outside
/// [`CONFLICT_RESOLUTION_VALUES`].
pub fn unsupported_conflict_resolution(value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "migrationOptions.conflictResolution {value} is not supported. Supported values are \
             {}.",
            CONFLICT_RESOLUTION_VALUES.join(", ")
        )),
    }
}

/// The `ValidationException` payload for a migration-status filter outside
/// [`MIGRATION_STATUS_VALUES`].
///
/// "Filters the results by migration status. Valid values are PENDING, IN_PROGRESS,
/// SUCCEEDED, and FAILED" (`ListMigrationsRequest$status`): a value outside the
/// published set names no status a job is ever in, so it is refused as the malformed
/// input it is rather than answered with an empty page.
pub fn unsupported_migration_status(value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "status {value} is not supported. Supported values are {}.",
            MIGRATION_STATUS_VALUES.join(", ")
        )),
    }
}

/// The same missing-application statement as [`application_not_found`], carried by the
/// client-input error an operation that models NO `ResourceNotFoundException` does
/// model.
///
/// One wording for "this account holds no such application", whichever error channel
/// the asking operation has for it.
pub fn unknown_application_id() -> ValidationException {
    ValidationException {
        message: application_not_found().message,
    }
}

// ---------------------------------------------------------------------------
// Indexes
//
// An index is a sub-resource of a domain, the way a data source is:
// `CreateIndex` mints one and `GetIndex` reports it, both from the ONE [`Index`]
// record below, so the schema a create persisted is the schema a read answers
// with.
// ---------------------------------------------------------------------------

/// `IndexStatus`'s `CREATED` value — the status `CreateIndex` reports for the index
/// it minted. `IndexStatus` declares exactly `CREATED | UPDATED | DELETED`
/// ([`crate::ops::types::INDEX_STATUS_VALUES`]): a value per index call, so what the
/// member carries is which call was served, not a lifecycle state the resource moves
/// through.
pub const INDEX_STATUS_CREATED: &str = "CREATED";

/// How many indexes one domain may hold before a create is refused.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — the model
/// publishes none — and it is what stops a caller looping with fresh index names
/// from growing this process's state without end.
pub const INDEX_QUOTA: usize = 100;

/// The characters `IndexName`'s pattern forbids ANYWHERE in the name, from its
/// trailing character class `[^ ,:"+/*\\|?#><A-Z]` (the upper-case letters that
/// class also excludes are tested as a range rather than listed here).
const INDEX_NAME_FORBIDDEN: &str = " ,:\"+/*\\|?#><";

/// The characters that class forbids only as the FIRST character, from
/// `IndexName`'s leading class `[^_ ,:"+/*\\|?#><A-Z-]`: an index name may carry an
/// underscore or a hyphen, just not lead with one.
const INDEX_NAME_FORBIDDEN_FIRST: [char; 2] = ['_', '-'];

/// Whether `value` is an `IndexName`: pattern
/// `^(?!\.\.?$)[^_ ,:"+/*\\|?#><A-Z-][^ ,:"+/*\\|?#><A-Z]*$` with
/// `@length(min: 1, max: 255)`.
///
/// The request boundary enforces the length bound but not this pattern — its
/// negative lookahead is not a construct the boundary's regex engine compiles — so
/// an operation that takes an `IndexName` checks the shape itself rather than
/// storing a name no index could have. The same reasoning as [`is_guid`] and
/// [`is_arn`], and a SHAPE test only: whether the name resolves to a stored index is
/// the lookup's job.
pub fn is_index_name(value: &str) -> bool {
    // The lookahead: `.` and `..` are the two names the pattern refuses outright
    // (they are the relative path segments of the index's own on-disk directory).
    if value == "." || value == ".." {
        return false;
    }
    let forbidden =
        |ch: char| INDEX_NAME_FORBIDDEN.contains(ch) || ch.is_ascii_uppercase();
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        // The length bound already refuses the empty name; a handler that is handed
        // one anyway refuses it here rather than storing it.
        return false;
    };
    !forbidden(first) && !INDEX_NAME_FORBIDDEN_FIRST.contains(&first) && !characters.any(forbidden)
}

/// `IndexStatus`'s `UPDATED` value — the status `UpdateIndex` reports for the index
/// whose schema it rewrote, the sibling of [`INDEX_STATUS_CREATED`].
pub const INDEX_STATUS_UPDATED: &str = "UPDATED";

/// `IndexStatus`'s `DELETED` value — the status `DeleteIndex` reports for the index
/// it removed from its domain, the third sibling of [`INDEX_STATUS_CREATED`].
pub const INDEX_STATUS_DELETED: &str = "DELETED";

/// One OpenSearch index of a domain, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Index {
    /// `IndexName`. Unique within the domain, so the name IS the key the domain's
    /// map holds it under.
    pub index_name: String,
    /// The `IndexSchema` document this index carries, stored VERBATIM as the
    /// modelled document rather than as a parsed mirror of it: `GetIndex` answers
    /// with "the JSON schema of the index", which is this. The create persists the
    /// document it carried and `UpdateIndex` merges each later change onto it
    /// ([`merged_index_schema`]).
    pub schema: ::aws_smithy_types::Document,
    /// The documents this index holds, keyed by their `_id` — unique within the index,
    /// so the id IS the key. Written by [`ingest::index_document`], the one data-plane
    /// write into an index, and bounded by [`ingest::INDEX_DOCUMENT_QUOTA`]; deleting
    /// the index or its domain releases them.
    pub documents: BTreeMap<String, ingest::IndexedDocument>,
}

/// One index TEMPLATE of a domain: the schema every index whose name matches one of its
/// patterns is CREATED with.
///
/// "An index template is a way to tell OpenSearch how to configure an index when it is
/// created": a template applies only to an index created AFTER it was written, and never
/// to one that already exists, which is why it is read exactly where an index is minted
/// without a schema of its own ([`Domain::templated_schema`]).
#[derive(Debug, Clone)]
pub struct IndexTemplate {
    /// The template's name — unique on the domain, so the name IS the key.
    pub name: String,
    /// `index_patterns`: the index names this template applies to, each of which may
    /// carry the `*` and `?` wildcards.
    pub index_patterns: Vec<String>,
    /// `priority`: of two templates matching one index name the HIGHER priority wins, and
    /// a template naming none has priority 0.
    pub priority: i64,
    /// The schema an index this template applies to is created with, in the same
    /// `{"mappings": …, "settings": …}` form a schema is stored in
    /// ([`Index::schema`]), so a templated index and one a client created carry one shape.
    pub schema: ::aws_smithy_types::Document,
    /// The template document as it was written, which the template API reports back.
    pub definition: ::serde_json::Value,
}

/// The `IndexSchema` section an index's field mappings live in: `UpdateIndex`
/// carries "any changes to mappings, settings, and semantic enrichment
/// configuration", and it is the mapped fields under this one whose types are
/// settled once written.
pub const SCHEMA_MAPPINGS: &str = "mappings";

/// The member a mapped field names its field type in.
const MAPPING_TYPE: &str = "type";

/// The members a mapping document nests further field mappings under: an object
/// field's own sub-properties, and a field's additional multi-fields. Neither names
/// a field itself, so a field's path through them is the path of its ancestors and
/// its own name only.
const MAPPING_CONTAINERS: [&str; 2] = ["properties", "fields"];

/// How many values ONE index's schema may hold once an update has been merged onto
/// it, before that update is refused.
///
/// Like [`INDEX_QUOTA`] this is the emulator's OWN bound on the store — the model
/// publishes none. A create persists ONE document, so the request boundary bounds
/// what it stores; an update MERGES onto the document the index already carries, so
/// without this a caller looping with fresh members would grow this process's state
/// without end, one bounded request at a time.
pub const INDEX_SCHEMA_QUOTA: usize = 10_000;

/// How many values `schema` holds: the document itself, every member of every named
/// set in it and every element of every list in it.
///
/// What [`INDEX_SCHEMA_QUOTA`] bounds. Iterative rather than recursive, so a deeply
/// nested document costs no stack, and each pending entry is one borrow, so it costs
/// no heap beyond that either.
pub fn index_schema_values(schema: &::aws_smithy_types::Document) -> usize {
    use ::aws_smithy_types::Document;

    let mut values = 0_usize;
    let mut pending = vec![schema];
    while let Some(node) = pending.pop() {
        values = values.saturating_add(1);
        match node {
            Document::Object(members) => pending.extend(members.values()),
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    values
}

/// How deeply ONE index's schema may nest, before the request carrying it is refused.
///
/// Like [`INDEX_SCHEMA_QUOTA`] this is the emulator's OWN bound — the model publishes
/// none — and it is what keeps the cost of accepting a document tied to its SIZE: the
/// walks over a schema ([`merged_index_schema`], [`mapping_type_conflict`]) address a
/// member by its path, and this service's own bound on a path's length is what stops a
/// small request naming paths that cost more than the bytes it carried. It sits below
/// the depth the request boundary's own deserializer admits, so this is the bound a
/// caller meets, and comfortably above the nesting a mapping document needs (a search
/// engine's own limit on the object fields one field may nest under is an order of
/// magnitude smaller), so no schema a caller would write reaches it.
pub const INDEX_SCHEMA_MAX_DEPTH: usize = 100;

/// How deeply `schema` nests: 1 for a document carrying no set and no list, one more
/// for each level of nesting under it.
///
/// What [`INDEX_SCHEMA_MAX_DEPTH`] bounds. Iterative rather than recursive, so
/// measuring even a document past the bound costs no stack, and each pending entry is
/// one borrow and one counter.
pub fn index_schema_depth(schema: &::aws_smithy_types::Document) -> usize {
    use ::aws_smithy_types::Document;

    let mut deepest = 0_usize;
    let mut pending = vec![(1_usize, schema)];
    while let Some((depth, node)) = pending.pop() {
        deepest = deepest.max(depth);
        match node {
            Document::Object(members) => {
                pending.extend(members.values().map(|child| (depth.saturating_add(1), child)));
            }
            Document::Array(elements) => {
                pending.extend(elements.iter().map(|child| (depth.saturating_add(1), child)));
            }
            _ => {}
        }
    }
    deepest
}

/// The `ValidationException` payload for an `IndexSchema` nesting deeper than
/// [`INDEX_SCHEMA_MAX_DEPTH`]. It reports both numbers, the same way
/// [`index_schema_over_quota`] does, so a caller knows how much nesting to drop.
pub fn index_schema_too_deep(depth: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "IndexSchema nests {depth} levels deep, more than the maximum of \
             {INDEX_SCHEMA_MAX_DEPTH} for one index."
        )),
    }
}

/// The `ValidationException` payload for an update whose merged schema would hold
/// more values than [`INDEX_SCHEMA_QUOTA`]. It reports both numbers, so a caller
/// knows how much of the update to drop.
pub fn index_schema_over_quota(values: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "IndexSchema would hold {values} values, more than the maximum of \
             {INDEX_SCHEMA_QUOTA} for one index."
        )),
    }
}

/// A field whose mapped type an update would change: the field's dotted path within
/// the index's mappings, the type the index maps it with, and the type the update
/// names for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingTypeConflict {
    /// The field's path under `mappings`, the way a mapping document addresses a
    /// field of an object field (`author.name`).
    pub field: String,
    /// The type the stored schema maps the field with.
    pub from: String,
    /// The type the update names for it.
    pub to: String,
}

/// The mapped field whose type `update` would change against `stored`, or `None`
/// when the update changes no mapped field's type.
///
/// A field's type is settled by the write that first maps it: a later mapping of
/// the same field may add to it but not re-type it. Walking BOTH documents is what
/// makes that a property of the stored schema rather than of one request — a field
/// the stored schema does not map yet has no type to conflict with, and a field the
/// update does not mention is not touched.
///
/// Iterative rather than recursive, so a deeply nested schema costs no stack, and the
/// field path is held as a parent chain rather than on each pending entry, so a
/// pending entry costs one borrow and the whole walk one name per mapping node —
/// never a copy of the path per child. Only the SMALLEST conflict is kept, so the
/// walk's cost does not grow with how many fields conflict. The conflict it reports is
/// the least field path, so the same pair of documents always reports the same one. A
/// field literally named `properties` or `fields` is addressed by its parent's path,
/// since those two names are how a mapping document nests rather than field names.
pub fn mapping_type_conflict(
    stored: &::aws_smithy_types::Document,
    update: &::aws_smithy_types::Document,
) -> Option<MappingTypeConflict> {
    use ::aws_smithy_types::Document;

    let (Document::Object(stored_root), Document::Object(update_root)) = (stored, update) else {
        return None;
    };
    let (Some(Document::Object(stored_mappings)), Some(Document::Object(update_mappings))) = (
        stored_root.get(SCHEMA_MAPPINGS),
        update_root.get(SCHEMA_MAPPINGS),
    ) else {
        return None;
    };

    // The field names walked so far, each entry naming its parent entry: the path of
    // a node is read off this chain ([`mapping_field_path`]) only where a conflict has
    // to be reported, so no pending entry carries a path of its own.
    let mut names: Vec<(&str, Option<usize>)> = Vec::new();
    let mut least: Option<MappingTypeConflict> = None;
    let mut pending = vec![(None, stored_mappings, update_mappings)];
    while let Some((at, stored_node, update_node)) = pending.pop() {
        if at.is_some() {
            if let (Some(from), Some(to)) =
                (stored_node.get(MAPPING_TYPE), update_node.get(MAPPING_TYPE))
            {
                // Compared as documents rather than as type NAMES, so a mapping
                // carrying something other than a name where its type belongs is
                // still settled by the write that first made it rather than
                // rewritten by the next one.
                if from != to {
                    let conflict = MappingTypeConflict {
                        field: mapping_field_path(&names, at),
                        from: mapping_type_label(from),
                        to: mapping_type_label(to),
                    };
                    if least
                        .as_ref()
                        .is_none_or(|best| conflict.field < best.field)
                    {
                        least = Some(conflict);
                    }
                }
            }
        }
        // Descend only where BOTH documents carry a mapping node, which is where a
        // type the stored schema already settled can be contradicted.
        for (name, update_child) in update_node {
            let Document::Object(update_child) = update_child else {
                continue;
            };
            let Some(Document::Object(stored_child)) = stored_node.get(name) else {
                continue;
            };
            let child = if MAPPING_CONTAINERS.contains(&name.as_str()) {
                at
            } else {
                names.push((name.as_str(), at));
                Some(names.len() - 1)
            };
            pending.push((child, stored_child, update_child));
        }
    }
    least
}

/// The dotted path of the mapping node `at` names, read off the parent chain
/// [`mapping_type_conflict`] records its walk in — the way a mapping document
/// addresses a field of an object field (`author.name`).
fn mapping_field_path(names: &[(&str, Option<usize>)], at: Option<usize>) -> String {
    let mut path: Vec<&str> = Vec::new();
    let mut cursor = at;
    while let Some(index) = cursor {
        let (name, parent) = names[index];
        path.push(name);
        cursor = parent;
    }
    path.reverse();
    path.join(".")
}

/// How a field's `type` member reads in a refusal: the type name the mapping names,
/// or the value standing where a type name belongs.
fn mapping_type_label(value: &::aws_smithy_types::Document) -> String {
    use ::aws_smithy_types::{Document, Number};

    match value {
        Document::String(name) => name.clone(),
        Document::Bool(flag) => flag.to_string(),
        Document::Null => "null".to_string(),
        Document::Number(Number::PosInt(unsigned)) => unsigned.to_string(),
        Document::Number(Number::NegInt(signed)) => signed.to_string(),
        Document::Number(Number::Float(float)) => float.to_string(),
        Document::Array(_) => "a list".to_string(),
        Document::Object(_) => "a set of members".to_string(),
    }
}

/// `stored` with `update` merged onto it — the schema an accepted update leaves the
/// index carrying.
///
/// An update carries "any CHANGES to mappings, settings, and semantic enrichment
/// configuration", so every member it names is written and every member of the
/// stored schema it does not name stays. Only a member whose value is a named set
/// of further members is descended into: a scalar, a null and an array are single
/// values, so one of those replaces whatever the stored schema held there, and an
/// object that names no member changes nothing.
///
/// Iterative rather than recursive, so a deeply nested document costs no stack, and
/// the path of a member is held as a parent chain rather than on each pending entry,
/// so a pending entry costs one borrow and the whole walk one name per member — never
/// a copy of the path per child. With the depth of an accepted document bounded
/// ([`INDEX_SCHEMA_MAX_DEPTH`]) and its value count bounded
/// ([`INDEX_SCHEMA_QUOTA`]), the heap this needs is tied to the size of the request
/// that carried it.
pub fn merged_index_schema(
    stored: &::aws_smithy_types::Document,
    update: &::aws_smithy_types::Document,
) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::Document;

    if !matches!(stored, Document::Object(_)) || !matches!(update, Document::Object(_)) {
        return update.clone();
    }
    let mut merged = stored.clone();
    // The member names walked so far, each entry naming its parent entry: a value's
    // path is read off this chain where the value is written.
    let mut names: Vec<(&str, Option<usize>)> = Vec::new();
    let mut pending: Vec<(Option<usize>, &Document)> = vec![(None, update)];
    while let Some((at, node)) = pending.pop() {
        match node {
            // A named set of members names no value of its own: it is the way to the
            // members under it, so one that names none changes nothing.
            Document::Object(members) => {
                for (name, child) in members {
                    names.push((name.as_str(), at));
                    pending.push((Some(names.len() - 1), child));
                }
            }
            value => {
                let path = member_path(&names, at);
                write_member(&mut merged, &path, value.clone());
            }
        }
    }
    merged
}

/// The path of the member `at` names, root first, read off the parent chain
/// [`merged_index_schema`] records its walk in.
fn member_path<'a>(names: &[(&'a str, Option<usize>)], at: Option<usize>) -> Vec<&'a str> {
    let mut path: Vec<&str> = Vec::new();
    let mut cursor = at;
    while let Some(index) = cursor {
        let (name, parent) = names[index];
        path.push(name);
        cursor = parent;
    }
    path.reverse();
    path
}

/// Write `value` at `path` in `document`, as [`merged_index_schema`]'s one writing
/// step. Each node above it that is not a named set of members becomes one, so a
/// nested member is written without dropping the members beside it.
fn write_member(
    document: &mut ::aws_smithy_types::Document,
    path: &[&str],
    value: ::aws_smithy_types::Document,
) {
    use ::aws_smithy_types::Document;

    // The root itself is not a member, so a document that names nothing writes
    // nothing.
    let Some((name, ancestors)) = path.split_last() else {
        return;
    };
    let mut node = document;
    for ancestor in ancestors {
        if !matches!(node, Document::Object(_)) {
            *node = Document::Object(::std::collections::HashMap::new());
        }
        let Document::Object(members) = node else {
            return;
        };
        node = members
            .entry((*ancestor).to_string())
            .or_insert_with(|| Document::Object(::std::collections::HashMap::new()));
    }
    if !matches!(node, Document::Object(_)) {
        *node = Document::Object(::std::collections::HashMap::new());
    }
    if let Document::Object(members) = node {
        members.insert((*name).to_string(), value);
    }
}

/// The `IndexSchema` section an index's analysis settings live in: the tokenizers,
/// filters and analyzers the domain's cluster builds the index's text pipeline from.
///
/// Looked for wherever it appears rather than at one fixed depth, because the same
/// settings reach the cluster both directly under the index's settings and under the
/// `index` section inside them.
const SCHEMA_ANALYSIS: &str = "analysis";

/// The suffix of an analysis setting that names its content BY PATH rather than inline
/// (`synonyms_path`, `stopwords_path`, `rules_path`, …): the form every such file
/// reference takes in the engine's own settings, which is why the suffix rather than a
/// hand-written list of setting names is what identifies one.
const PATH_SETTING_SUFFIX: &str = "_path";

/// One file an index's analysis settings resolve by path on the cluster nodes, as the
/// index names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageReference {
    /// The analysis setting that carried the path (`synonyms_path`) — what a refusal
    /// names, because it is the setting whose file the cluster could not read.
    pub setting: String,
    /// The path the setting carries, relative to the cluster nodes' package directory
    /// — the same form as [`package_reference_path`].
    pub path: String,
    /// The `PackageID` the path names under the analyzers directory: the package that
    /// has to be in place on the domain for the file to be there to read.
    pub package_id: String,
}

impl PackageReference {
    /// Whether this reference resolves content the package `package_id` names provides:
    /// the package's own directory, or a file inside it.
    pub fn names_package(&self, package_id: &str) -> bool {
        self.package_id == package_id
    }
}

/// Every package file `schema`'s analysis settings name by path.
///
/// An index's analysis settings reference a custom dictionary package's content BY the
/// relative path the package is mounted at ([`package_reference_path`]), so this walks
/// the schema's analysis sections and reads out each path setting that names one.
/// Ordered by setting name, so the same document always reports the same first
/// reference, and iterative rather than recursive, so a deeply nested document costs no
/// stack.
pub fn package_references(schema: &::aws_smithy_types::Document) -> Vec<PackageReference> {
    use ::aws_smithy_types::Document;

    // The analysis sections first: a path setting outside one names no analyzer's
    // content, so only what is under them is read out.
    let mut analysis: Vec<&Document> = Vec::new();
    let mut pending = vec![schema];
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (name, child) in members {
                    if name == SCHEMA_ANALYSIS {
                        analysis.push(child);
                    } else {
                        pending.push(child);
                    }
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }

    let mut references: Vec<PackageReference> = Vec::new();
    let mut pending = analysis;
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (name, child) in members {
                    match child {
                        Document::String(value) if name.ends_with(PATH_SETTING_SUFFIX) => {
                            if let Some(package_id) = referenced_package(value) {
                                references.push(PackageReference {
                                    setting: name.clone(),
                                    path: value.clone(),
                                    package_id,
                                });
                            }
                        }
                        other => pending.push(other),
                    }
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    references.sort_by(|left, right| left.setting.cmp(&right.setting));
    references
}

/// The `PackageID` `path` names under the analyzers directory, or `None` where the path
/// names no package's content at all.
///
/// The documented form of such a reference is the analyzers directory followed by the
/// package's id ([`package_reference_path`]), optionally followed by a file inside it,
/// so the id is the segment after the directory.
fn referenced_package(path: &str) -> Option<String> {
    let rest = path.strip_prefix(ANALYZERS_DIRECTORY)?.strip_prefix('/')?;
    let package_id = rest.split('/').next()?;
    (!package_id.is_empty()).then(|| package_id.to_string())
}

/// The `ValidationException` payload for an index whose analysis settings name a file
/// the cluster cannot read, because the domain holds no association in place with the
/// package that provides it.
///
/// The cluster builds the index's analysis pipeline by READING each file its settings
/// name, so the refusal is the failure to read that one setting's file, reported the way
/// the engine reports it: the setting it was reading, followed by the suffix the engine
/// appends to whatever setting it was handed, and what went wrong. The managed layer
/// hands it the member key it found in the schema, so a member a request carries as
/// `<name>_path` is reported as `<name>_path_path` — the engine's own formatting applied
/// to the request's own member name. The only observed sentence of this class is for a
/// synonym filter's path setting; whether the managed layer hands the engine the member
/// key for EVERY path setting is not established, so the wording for the other path
/// settings follows from this one derivation rather than from an observation of each.
pub fn package_file_not_readable(setting: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "IOException while reading {setting}{PATH_SETTING_SUFFIX}: file not readable"
        )),
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

/// `InboundConnectionStatusCode`'s (and `OutboundConnectionStatusCode`'s) `APPROVED`:
/// where the destination domain owner's acceptance puts a connection that was waiting
/// for their decision, before the connection itself is in place.
pub const CONNECTION_STATUS_APPROVED: &str = "APPROVED";

/// `InboundConnectionStatusCode`'s `PROVISIONING`: "Inbound connection is being
/// provisioned" — the step between an approved request and a usable connection.
pub const CONNECTION_STATUS_PROVISIONING: &str = "PROVISIONING";

/// `InboundConnectionStatusCode`'s `ACTIVE`: "Inbound connection is active and ready
/// to use" — where an approved connection ends up once it is in place.
pub const CONNECTION_STATUS_ACTIVE: &str = "ACTIVE";

/// `InboundConnectionStatusCode`'s `REJECTING`: "Inbound connection rejection is in
/// process" — where the destination domain owner's refusal of a request puts it.
pub const CONNECTION_STATUS_REJECTING: &str = "REJECTING";

/// `InboundConnectionStatusCode`'s `REJECTED`: "Inbound connection is rejected" —
/// where a refused request comes to rest.
pub const CONNECTION_STATUS_REJECTED: &str = "REJECTED";

/// `InboundConnectionStatusCode`'s (and `OutboundConnectionStatusCode`'s) `DELETING`:
/// "Inbound connection deletion is in progress" — where a delete leaves the record
/// while the connection is being taken down.
pub const CONNECTION_STATUS_DELETING: &str = "DELETING";

/// `InboundConnectionStatusCode`'s `DELETED`: "Inbound connection is deleted and can
/// no longer be used" — where a deleted connection comes to rest, and goes on being
/// reported from and addressable by its id.
pub const CONNECTION_STATUS_DELETED: &str = "DELETED";

/// How many DELETED cross-cluster connection records the store keeps beside the
/// connections the account still holds.
///
/// A deleted connection comes to rest in [`CONNECTION_STATUS_DELETED`] and its id goes
/// on answering, so a caller that re-issues a delete after a timeout is told the
/// connection it deleted is deleted rather than that the id was never its own. Keeping
/// those records is what makes that true, and this is the bound on keeping them: the
/// oldest is released once the store holds more
/// ([`ComAmazonawsOpensearchAmazonopensearchserviceState::trim_connection_history`]),
/// so the record set cannot grow with the number of deletes a caller makes. Set at
/// the aggregate bound on held connections, so the whole store stays within twice
/// [`CONNECTION_STORE_QUOTA`] records.
pub const CONNECTION_HISTORY: usize = CONNECTION_STORE_QUOTA;

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
/// record behind both views — `CreateOutboundConnection` mints it,
/// `DescribeOutboundConnections` reports it as [`Connection::to_outbound`] and
/// `DescribeInboundConnections` reports it as [`Connection::to_inbound`] — so no two
/// operations can report the same connection differently.
#[derive(Debug, Clone)]
pub struct Connection {
    /// `OutboundConnection$ConnectionId`, which is also the inbound view's. Unique
    /// per connection, so it IS the key the store holds the record under.
    pub connection_id: String,
    /// The order the store minted this record in.
    ///
    /// The id is an opaque token, so it does not say which of two records is the
    /// older one; this does, which is what lets the store release the OLDEST deleted
    /// record when it keeps more history than [`CONNECTION_HISTORY`].
    pub sequence: u64,
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
    /// The status the DESTINATION owner's view of this connection has come to rest
    /// in for good, where that is no longer the status the shared lifecycle reports;
    /// `None` while both views report the one lifecycle.
    ///
    /// The two views of a connection share a lifecycle until a decision the
    /// destination owner took is a record only THAT owner keeps: refusing a request
    /// is the destination owner's permanent record of the decision, so the source
    /// owner deleting its own view of the connection leaves that record exactly where
    /// it was rather than carrying it along to the deleted status
    /// ([`Connection::begin_delete`]).
    pub inbound_rest: Option<String>,
}

impl Connection {
    /// Materialise the status move the clock owes this connection, if `now` is past
    /// its due instant: a request that was being validated becomes one waiting for
    /// the remote domain owner's decision, or one whose validation failed.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called.
    ///
    /// A status the clock owes may itself owe the next one ([`connection_successor`]),
    /// so the moves are applied in a loop from the instant each FELL DUE rather than
    /// from `now`: a caller that reads a connection long after it was accepted sees
    /// the status it would have reached without them, not one step of its lifecycle
    /// per call.
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(due) = self.owed.as_ref().map(|owed| owed.due) {
            if now < due {
                return;
            }
            let Some(owed) = self.owed.take() else {
                return;
            };
            self.status = owed.status;
            self.status_message = owed.message;
            self.owed = connection_successor(&self.status).map(|status| OwedStatus {
                status: status.to_string(),
                message: None,
                due: due + PROVISIONING,
            });
        }
    }

    /// Whether this connection has been deleted: the delete a caller asked for has
    /// been carried out and the record is the account's record OF the deletion rather
    /// than a connection it holds.
    ///
    /// What a deleted connection no longer holds is everything a live one does — the
    /// alias its source domain named it by and the slot it counted against
    /// [`CONNECTION_QUOTA`] — so a source domain can connect to the same destination
    /// under the same alias again.
    pub fn deleted(&self) -> bool {
        self.status == CONNECTION_STATUS_DELETED
    }

    /// Put this connection on its way out of the store, unless a delete already did.
    ///
    /// Taking the connection down is work the service does, so the record is in its
    /// deleting status when the call that asked for this returns and the clock owes it
    /// the deleted status it comes to rest in ([`connection_successor`]). A delete of
    /// a connection a delete already took does not restart that: the record stands as
    /// it is, which is what makes a repeat delete of an id this caller deleted the
    /// no-op it reports rather than a second deletion of one connection.
    ///
    /// The two views follow the deletion together only where both are views of a LIVE
    /// connection. Where the destination owner's side has already come to rest on its
    /// own account — it refused the request, so its record is the record OF that
    /// refusal — or where it never had a view of the connection at all, the inbound
    /// view stays exactly where it was ([`Connection::inbound_rest`]): deleting the
    /// source owner's view of a connection is not the destination owner taking a
    /// decision back.
    pub fn begin_delete(&mut self, now: SystemTime) {
        if self.status == CONNECTION_STATUS_DELETING || self.deleted() {
            return;
        }
        if !self.has_inbound_view() || self.status == CONNECTION_STATUS_REJECTED {
            self.inbound_rest = Some(self.status.clone());
        }
        self.status = CONNECTION_STATUS_DELETING.to_string();
        self.status_message = None;
        self.owed = Some(OwedStatus {
            status: CONNECTION_STATUS_DELETED.to_string(),
            message: None,
            due: now + PROVISIONING,
        });
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

    /// Where the DESTINATION owner's view of this connection is: the status its own
    /// side came to rest in for good, where that is no longer the shared lifecycle's
    /// ([`Connection::inbound_rest`]), and the shared lifecycle's status otherwise.
    ///
    /// Every operation on the destination owner's view of a connection decides from
    /// this rather than from [`Connection::status`], so what that owner is told about
    /// the connection and what it may do with it come from the same status.
    pub fn inbound_status(&self) -> &str {
        self.inbound_rest.as_deref().unwrap_or(&self.status)
    }

    /// Whether this connection has an INBOUND view at all — whether the destination
    /// domain's owner has anything to see or decide about it yet.
    ///
    /// The two views of one connection share a lifecycle, but not every status the
    /// source owner's outbound view reports is one the destination owner's inbound
    /// view can be in: `InboundConnectionStatusCode` carries neither
    /// [`CONNECTION_STATUS_VALIDATING`] nor [`CONNECTION_STATUS_VALIDATION_FAILED`],
    /// while `OutboundConnectionStatusCode` carries both. A request that is still
    /// being validated, or whose validation failed, therefore has no inbound view —
    /// it is reported to the source owner only, and the status set the inbound shape
    /// models is what says so.
    pub fn has_inbound_view(&self) -> bool {
        INBOUND_CONNECTION_STATUS_CODE_VALUES.contains(&self.inbound_status())
    }

    /// This record as `InboundConnection` — the element shape
    /// `DescribeInboundConnections` reports, and the projection every operation on
    /// the destination owner's view of a connection answers with.
    ///
    /// The same stored record [`Connection::to_outbound`] projects, so the two views
    /// of one connection cannot disagree about its id, its endpoints or its mode, and
    /// they report one lifecycle for as long as the connection has one
    /// ([`Connection::inbound_status`]). The inbound shape models no `ConnectionAlias`
    /// and no `ConnectionProperties`: the alias is how the SOURCE domain names the
    /// remote cluster, so it is the source owner's view that carries it.
    pub fn to_inbound(&self) -> InboundConnection {
        InboundConnection {
            local_domain_info: Some(domain_information(&self.local_domain)),
            remote_domain_info: Some(domain_information(&self.remote_domain)),
            connection_id: Some(self.connection_id.clone()),
            connection_status: Some(InboundConnectionStatus {
                status_code: Some(self.inbound_status().to_string()),
                // "Information about the connection": carried only where the status
                // has something to say the code itself does not.
                message: self.status_message.clone(),
            }),
            connection_mode: Some(self.connection_mode.clone()),
        }
    }
}

/// The status a connection moves to ON ITS OWN once it has reached `status`, and
/// `None` where it is somewhere only another call takes it further from.
///
/// A connection's lifecycle has steps the service drives and steps a caller drives:
/// once a domain owner has decided about a request, carrying that decision out is
/// work the service does, so each of those steps falls due [`PROVISIONING`] after the
/// previous one rather than waiting for another call. Where a connection waits for a
/// decision — a validated request waiting to be accepted or rejected — or where it
/// has come to rest, nothing is owed.
pub fn connection_successor(status: &str) -> Option<&'static str> {
    match status {
        CONNECTION_STATUS_PROVISIONING => Some(CONNECTION_STATUS_ACTIVE),
        CONNECTION_STATUS_REJECTING => Some(CONNECTION_STATUS_REJECTED),
        CONNECTION_STATUS_DELETING => Some(CONNECTION_STATUS_DELETED),
        _ => None,
    }
}

/// A stored domain reference in the container the wire reports it in.
pub fn domain_information(info: &AWSDomainInformation) -> DomainInformationContainer {
    DomainInformationContainer {
        aws_domain_information: Some(info.clone()),
    }
}

/// The values of a domain's `DomainConfig` options, as ONE snapshot: the
/// configuration a change started FROM, which is what cancelling that change puts
/// back.
///
/// Every member is the [`Domain`] field holding one option an `UpdateDomainConfig`
/// request can set, so a snapshot carries the whole configuration a change could
/// have moved and nothing else: the domain's identity, its history and the
/// resources it holds are not options and are never restored.
#[derive(Debug, Clone)]
pub struct DomainOptions {
    cluster_config: ClusterConfig,
    ebs_options: Option<EBSOptions>,
    access_policies: Option<String>,
    ip_address_type: Option<String>,
    snapshot_options: Option<SnapshotOptions>,
    vpc_options: Option<VPCDerivedInfo>,
    cognito_options: Option<CognitoOptions>,
    encryption_at_rest_options: Option<EncryptionAtRestOptions>,
    node_to_node_encryption_options: Option<NodeToNodeEncryptionOptions>,
    advanced_options: BTreeMap<String, String>,
    log_publishing_options: BTreeMap<String, LogPublishingOption>,
    domain_endpoint_options: Option<DomainEndpointOptions>,
    advanced_security_options: Option<AdvancedSecurityOptions>,
    identity_center_options: Option<IdentityCenterOptions>,
    auto_tune_options: Option<AutoTuneOptions>,
    off_peak_window_options: Option<OffPeakWindowOptions>,
    software_update_options: Option<SoftwareUpdateOptions>,
    aiml_options: Option<AIMLOptionsOutput>,
    deployment_strategy_options: Option<DeploymentStrategyOptions>,
    automated_snapshot_pause_options: Option<AutomatedSnapshotPauseOptions>,
    use_case: String,
    engine_mode: String,
}

impl DomainOptions {
    /// The option values `domain` holds right now.
    pub fn capture(domain: &Domain) -> Self {
        Self {
            cluster_config: domain.cluster_config.clone(),
            ebs_options: domain.ebs_options.clone(),
            access_policies: domain.access_policies.clone(),
            ip_address_type: domain.ip_address_type.clone(),
            snapshot_options: domain.snapshot_options.clone(),
            vpc_options: domain.vpc_options.clone(),
            cognito_options: domain.cognito_options.clone(),
            encryption_at_rest_options: domain.encryption_at_rest_options.clone(),
            node_to_node_encryption_options: domain.node_to_node_encryption_options.clone(),
            advanced_options: domain.advanced_options.clone(),
            log_publishing_options: domain.log_publishing_options.clone(),
            domain_endpoint_options: domain.domain_endpoint_options.clone(),
            advanced_security_options: domain.advanced_security_options.clone(),
            identity_center_options: domain.identity_center_options.clone(),
            auto_tune_options: domain.auto_tune_options.clone(),
            off_peak_window_options: domain.off_peak_window_options.clone(),
            software_update_options: domain.software_update_options.clone(),
            aiml_options: domain.aiml_options.clone(),
            deployment_strategy_options: domain.deployment_strategy_options.clone(),
            automated_snapshot_pause_options: domain.automated_snapshot_pause_options.clone(),
            use_case: domain.use_case.clone(),
            engine_mode: domain.engine_mode.clone(),
        }
    }

    /// Put these values back on `domain`, leaving everything that is not one of its
    /// options alone.
    pub fn restore(self, domain: &mut Domain) {
        domain.cluster_config = self.cluster_config;
        domain.ebs_options = self.ebs_options;
        domain.access_policies = self.access_policies;
        domain.ip_address_type = self.ip_address_type;
        domain.snapshot_options = self.snapshot_options;
        domain.vpc_options = self.vpc_options;
        domain.cognito_options = self.cognito_options;
        domain.encryption_at_rest_options = self.encryption_at_rest_options;
        domain.node_to_node_encryption_options = self.node_to_node_encryption_options;
        domain.advanced_options = self.advanced_options;
        domain.log_publishing_options = self.log_publishing_options;
        domain.domain_endpoint_options = self.domain_endpoint_options;
        domain.advanced_security_options = self.advanced_security_options;
        domain.identity_center_options = self.identity_center_options;
        domain.auto_tune_options = self.auto_tune_options;
        domain.off_peak_window_options = self.off_peak_window_options;
        domain.software_update_options = self.software_update_options;
        domain.aiml_options = self.aiml_options;
        domain.deployment_strategy_options = self.deployment_strategy_options;
        domain.automated_snapshot_pause_options = self.automated_snapshot_pause_options;
        domain.use_case = self.use_case;
        domain.engine_mode = self.engine_mode;
    }

    /// `option`'s value as the string `CancelledChangeProperty$CancelledValue` and
    /// `$ActiveValue` are, for an option whose value IS one.
    ///
    /// `None` for the structured containers and the maps: how this service renders
    /// one of them as a single string is not published, and an unpublished rendering
    /// is not invented — the property is then reported by name alone.
    pub fn option_value(&self, option: &str) -> Option<String> {
        match option {
            OPT_ACCESS_POLICIES => self.access_policies.clone(),
            OPT_IP_ADDRESS_TYPE => self.ip_address_type.clone(),
            OPT_USE_CASE => Some(self.use_case.clone()),
            OPT_ENGINE_MODE => Some(self.engine_mode.clone()),
            _ => None,
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
    /// The option values the domain held BEFORE this change, for a change a cancel
    /// can put back — recorded by [`Domain::record_revert`] and `None` for a change
    /// there is no going back from (the one a create starts, an engine version
    /// upgrade), which is therefore not cancellable.
    pub revert_to: Option<DomainOptions>,
    /// Whether this change was cancelled before it landed.
    pub cancelled: bool,
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

    /// Whether cancelling this change is something the service can do: it is still
    /// being applied AND the configuration it started from was recorded, so
    /// cancelling it puts that configuration back rather than leaving the change
    /// applied under another name.
    pub fn cancellable(&self) -> bool {
        self.in_progress() && self.revert_to.is_some()
    }

    /// `ChangeProgressStatusDetails$Status`: one of `OverallChangeStatus`'s values.
    pub fn status(&self) -> &'static str {
        if self.in_progress() {
            CHANGE_STATUS_PROCESSING
        } else if self.cancelled {
            CHANGE_STATUS_FAILED
        } else {
            CHANGE_STATUS_COMPLETED
        }
    }

    /// `ConfigChangeStatus` for this change: the emulator applies a change in one
    /// step, so it is `ApplyingChanges` until the change falls due and `Completed`
    /// after — or `Cancelled` where a cancel stopped it before it landed.
    pub fn config_change_status(&self) -> &'static str {
        if self.in_progress() {
            CONFIG_CHANGE_APPLYING_CHANGES
        } else if self.cancelled {
            CONFIG_CHANGE_CANCELLED
        } else {
            CONFIG_CHANGE_COMPLETED
        }
    }

    /// This change as `ChangeProgressStatusDetails` — what
    /// `DescribeDomainChangeProgress` answers with.
    pub fn to_progress_status(&self) -> ChangeProgressStatusDetails {
        // A property of the change in flight is pending; one of a change that has
        // landed is completed. The two lists partition `properties`, so no property
        // is ever reported as both — and a cancelled change landed none of them, so
        // neither list holds any.
        let (pending, completed) = match (self.in_progress(), self.cancelled) {
            (true, _) => (self.properties.clone(), Vec::new()),
            (false, true) => (Vec::new(), Vec::new()),
            (false, false) => (Vec::new(), self.properties.clone()),
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
            // dry run runs, which reports them on the dry run's OWN record
            // ([`DryRun::to_progress_status`], read by `DescribeDryRunProgress`)
            // rather than on the change a later call applied.
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

/// How many verbose dry runs one domain keeps, the same bound and for the same
/// reason as [`CHANGE_HISTORY`]: a caller may ask for one before every update it is
/// considering, so the record set a domain holds has to be bounded.
pub const DRY_RUN_HISTORY: usize = 50;

/// `DryRunMode`'s `Verbose` value (one of [`DRY_RUN_MODE_VALUES`]): the mode that
/// "runs an additional check to validate the changes you're making", which is the
/// analysis `DescribeDryRunProgress` reports the progress of. `Basic` "only returns
/// the type of deployment (blue/green or dynamic) that the update will cause", which
/// the update call answers from its own analysis, so it starts no run and leaves
/// nothing to report on.
pub const DRY_RUN_MODE_VERBOSE: &str = "Verbose";

/// `DryRunProgressStatus$DryRunStatus` while the validation check the dry run runs
/// has not finished — the status the call that started it answers with, so a caller
/// polling straight after it still observes the analysis in flight.
pub const DRY_RUN_STATUS_PENDING: &str = "PENDING";

/// `DryRunProgressStatus$DryRunStatus` once that check has finished.
pub const DRY_RUN_STATUS_COMPLETED: &str = "COMPLETED";

/// One verbose dry run on a domain: the analysis an `UpdateDomainConfig` carrying
/// `DryRun` with `DryRunMode` [`DRY_RUN_MODE_VERBOSE`] started, which
/// `DescribeDryRunProgress` reports the progress, the configuration and the results
/// of.
///
/// A dry run applies NOTHING: it is "a pre-update dry run analysis", so the record
/// holds the configuration the change WOULD have left the domain in — computed by
/// the same merge the real update performs, on a copy of the domain — beside the
/// deployment kind that change would need. It is [`DRY_RUN_STATUS_PENDING`] from the
/// call that started it until the span the clock owes the analysis has passed
/// ([`PROVISIONING`]), and
/// [`DRY_RUN_STATUS_COMPLETED`] after — materialised by whichever later call reads
/// the record ([`Self::settle`]), so nothing sleeps.
#[derive(Debug, Clone)]
pub struct DryRun {
    /// `DryRunProgressStatus$DryRunId`: the GUID this run is addressed by.
    pub dry_run_id: String,
    /// `CreationDate`: the instant of the call that started the run.
    pub started_at: SystemTime,
    /// `UpdateDate`: the instant this run's status last moved.
    pub last_updated_at: SystemTime,
    /// When the clock owes this run its conclusion; `None` once it has concluded.
    pub completes_at: Option<SystemTime>,
    /// The `DomainStatus` of the configuration the change would leave the domain in
    /// — "details about the changes you're planning to make on the domain", which
    /// `DescribeDryRunProgress` answers as `DryRunConfig`.
    ///
    /// A projection rather than a domain record, because it describes a
    /// configuration that was never applied and never will be by this run: there is
    /// no live domain for it to be re-derived from, so it is held exactly as the
    /// analysis computed it.
    pub config: DomainStatus,
    /// `DryRunResults$DeploymentType`: which of the four documented deployment kinds
    /// applying the change would be, as the update path's own analysis decided.
    pub deployment_type: String,
    /// `AcceptedWarnings`: "the list of advisory warning codes that were accepted
    /// for the configuration change", as the request that started the run carried
    /// them.
    pub accepted_warnings: Vec<String>,
}

impl DryRun {
    /// Materialise the conclusion the clock owes this run, if `now` is past its due
    /// instant.
    ///
    /// Dated at the DUE instant rather than at `now`, for the same reason a
    /// configuration change's completion is ([`ConfigChange::settle`]): the
    /// `UpdateDate` a caller reads does not move with the moment it happened to look.
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

    /// Whether the analysis is still running.
    pub fn in_progress(&self) -> bool {
        self.completes_at.is_some()
    }

    /// `DryRunProgressStatus$DryRunStatus` for this run.
    pub fn status(&self) -> &'static str {
        if self.in_progress() {
            DRY_RUN_STATUS_PENDING
        } else {
            DRY_RUN_STATUS_COMPLETED
        }
    }

    /// This run as `DryRunProgressStatus` — what the call that starts it and
    /// `DescribeDryRunProgress` both answer with, so the two can never report one run
    /// differently.
    pub fn to_progress_status(&self) -> DryRunProgressStatus {
        DryRunProgressStatus {
            dry_run_id: self.dry_run_id.clone(),
            dry_run_status: self.status().to_string(),
            creation_date: datetime_text(self.started_at),
            update_date: datetime_text(self.last_updated_at),
            // "The validation failures that occurred as a result of the dry run": a
            // configuration this service cannot apply is refused by the update path
            // itself, under the modelled error class for the rule it broke, and only
            // a configuration that passed every one of those checks reaches a dry
            // run — so a run that concluded found no failure, and one still running
            // has found none yet.
            validation_failures: Some(Vec::new()),
            accepted_warnings: Some(self.accepted_warnings.clone()),
        }
    }

    /// This run as `DryRunResults` — the same answer the call that started it gave,
    /// from the same stored analysis.
    pub fn to_results(&self) -> DryRunResults {
        DryRunResults {
            deployment_type: Some(self.deployment_type.clone()),
            // "A message corresponding to the deployment type": this service's own
            // wording for one is not published, and the deployment kind is already
            // reported in `DeploymentType`, so none is invented.
            message: None,
        }
    }
}

/// A service software deployment in flight on a domain: the release it is moving the
/// domain onto, and when the clock owes it completion.
///
/// A deployment is started by a service-software call and completes when the span the
/// clock owes it has passed ([`DOMAIN_WORK`], the same span a configuration change
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

/// `RollbackServiceSoftwareOptions$Description` for a domain that has no earlier
/// release to return to: the service's own sentence for that decline, which names
/// the reason (no previous version) and where to take it further.
pub const NO_ROLLBACK_TARGET_DESCRIPTION: &str =
    "No previous software version available for rollback. Please contact AWS Support \
     for assistance.";

/// How a descriptor of a domain names the domain it describes: itself. A refusal
/// names it by the name the request carried instead, which is the only difference
/// between the two statements of the sentences below.
const THIS_DOMAIN: &str = "this domain";

/// How the service states that a domain has no service software update to apply.
///
/// `ServiceSoftwareOptions$Description` states it about the domain being described
/// and a refused `StartServiceSoftwareUpdate` states it about the domain the request
/// named, so the sentence is written ONCE and only `domain` differs.
fn no_update_available_sentence(domain: &str) -> String {
    format!("There is no software update available for {domain}.")
}

/// The same for a domain that is already applying a deployment.
fn deployment_in_progress_sentence(domain: &str) -> String {
    format!("A service software deployment is already in progress on {domain}.")
}

/// The same for a domain that has an update scheduled and not yet begun.
fn scheduled_update_sentence(domain: &str) -> String {
    format!("A service software update is already scheduled on {domain}.")
}

/// How the service states that a domain cannot take a service software update now:
/// a deployment is already in flight on it, one is already scheduled, no release is
/// available for it to be moved onto, or other work is in flight on the domain.
///
/// ONE sentence for the whole rule, with no variable part — the service states the
/// condition, not which part of it the record failed, and not the domain the request
/// named.
pub const DOMAIN_NOT_AVAILABLE_FOR_UPDATE: &str = "The domain is not available for update.";

/// The `ValidationException` payload for a service software update a domain is not in
/// a state to take.
///
/// `StartServiceSoftwareUpdate` models `ValidationException` for a request the
/// service cannot serve as asked, and a domain that is not available for an update is
/// exactly that — there is no release to deploy, so nothing is scheduled.
pub fn domain_not_available_for_update() -> ValidationException {
    ValidationException {
        message: Some(DOMAIN_NOT_AVAILABLE_FOR_UPDATE.to_string()),
    }
}

/// How the service states that a domain has no service software update a cancel can
/// call off: both halves of the answer — that the cancel cannot be done, and why —
/// in one fixed sentence carrying no variable part.
pub const NO_PENDING_UPDATE_TO_CANCEL: &str =
    "The domain update cannot be cancelled. There is no pending update.";

/// The `ValidationException` payload for a service software update a domain has none
/// of to call off: nothing is scheduled on it, or the deployment it scheduled has
/// already begun — and a deployment under way is past the point a cancel can reach.
///
/// `CancelServiceSoftwareUpdate` is documented "you can only perform this operation
/// before the `AutomatedUpdateDate` and when the domain's `UpdateStatus` is
/// `PENDING_UPDATE`", so outside that state there is nothing to cancel and the call is
/// refused with the operation's modelled `ValidationException`.
pub fn no_software_update_to_cancel() -> ValidationException {
    ValidationException {
        message: Some(NO_PENDING_UPDATE_TO_CANCEL.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The refusals a domain with work in flight answers with
//
// A domain that is being provisioned, applying a configuration change, upgrading
// its engine or deploying service software ([`Domain::work_in_flight`]) serves
// neither a report of its nodes nor a call that would start further work on it.
// Each operation refuses with the error ITS model declares for the condition and
// with the service's own fixed sentence for it, which is why the sentences live
// here beside one another rather than in the handlers.
// ---------------------------------------------------------------------------

/// How the service states that the node information of a domain cannot be fetched
/// right now, and that the condition passes.
pub const NODE_INFORMATION_UNAVAILABLE: &str =
    "The node information is not available now, please try again later.";

/// The `DependencyFailureException` payload for a report of a domain's nodes the
/// service cannot answer: the nodes of a domain with work in flight are not up, or
/// are being replaced, so there is no node census to read.
///
/// `DependencyFailureException` is "an exception for when a failure in one of the
/// dependencies results in the service being unable to fetch details about the
/// resource" (`@httpError 424`), which is that condition exactly.
pub fn node_information_unavailable() -> DependencyFailureException {
    DependencyFailureException {
        message: Some(NODE_INFORMATION_UNAVAILABLE.to_string()),
    }
}

/// The `BaseException` payload for the additional validation check a verbose dry run
/// runs against the live domain, which cannot run while the domain is busy.
pub fn verbose_dry_run_during_activity() -> BaseException {
    BaseException {
        message: Some(
            "Verbose Dry Run cannot be performed at this time due to ongoing activities on \
             the domain"
                .to_string(),
        ),
    }
}

/// The `BaseException` payload for an engine version upgrade asked for on a domain
/// that is already busy with work of its own.
pub fn upgrade_blocked_by_background_tasks() -> BaseException {
    BaseException {
        message: Some("Upgrade failed. Domain has background tasks in progress".to_string()),
    }
}

/// The `ValidationException` payload for a call that would start another change on a
/// domain while one is still being applied.
pub fn change_already_in_progress() -> ValidationException {
    ValidationException {
        message: Some(
            "A change/update is in progress. Please wait for it to complete before \
             requesting another change."
                .to_string(),
        ),
    }
}

/// The `ValidationException` payload for a cancel asked for outside the two states a
/// domain has a change to cancel in, naming them.
pub fn cancel_outside_cancellable_states() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "CancelDomainConfigChange only when domain state is in \
             {PROCESSING_STATUS_MODIFYING} or {PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE}."
        )),
    }
}

/// The two `DomainProcessingStatusType` values a domain has a configuration change to
/// cancel in: one applying a change of its options, and one deploying service
/// software. Every other state — being created, deleted, upgraded, or settled — has
/// no such change, which is what [`cancel_outside_cancellable_states`] states.
pub const CANCELLABLE_PROCESSING_STATUSES: [&str; 2] = [
    PROCESSING_STATUS_MODIFYING,
    PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE,
];

/// A service software update a caller asked to start LATER: the release it will move
/// the domain onto, and the instant the schedule the caller named puts it at.
///
/// An update asked for at an instant still ahead has been REQUESTED and has not begun,
/// which is `DeploymentStatus`'s `PENDING_UPDATE` and the one state
/// `ServiceSoftwareOptions$Cancellable` ("true if you're able to cancel your service
/// software version update") is true in. It becomes a [`SoftwareDeployment`] when the
/// clock reaches `starts_at` — materialised by whichever later call reads the record,
/// so nothing sleeps.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// The release the domain will be moved onto, reported as
    /// `ServiceSoftwareOptions$NewVersion` while the update waits.
    pub target_version: String,
    /// When the schedule the request named has the update begin.
    pub starts_at: SystemTime,
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
    /// The update a caller asked to start at a later instant, if any: requested, not
    /// begun, and so the one thing a cancel could still call off.
    pub pending_update: Option<PendingUpdate>,
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
            pending_update: None,
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
    /// An update asked for at a later instant is begun here too, once the clock has
    /// reached it: that deployment was owed at the same instant whether or not anybody
    /// called.
    ///
    /// The landing is dated at the DUE instant rather than at `now`, so the rollback
    /// window a caller reads does not move with the moment it happened to look.
    fn settle(&mut self, now: SystemTime) {
        // An update is begun before any landing is considered, so one whose whole span
        // has passed between two reads both begins and lands in a single settling.
        if self
            .pending_update
            .as_ref()
            .is_some_and(|pending| now >= pending.starts_at)
        {
            if let Some(pending) = self.pending_update.take() {
                self.deployment = Some(SoftwareDeployment {
                    completes_at: pending
                        .starts_at
                        .checked_add(DOMAIN_WORK)
                        .unwrap_or(pending.starts_at),
                    target_version: pending.target_version,
                    rollback: false,
                });
            }
        }
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

    /// Whether a service software update is available on this domain: a release it can
    /// be moved onto is available, no deployment is in flight to interrupt and none is
    /// waiting to begin.
    ///
    /// This is `ServiceSoftwareOptions$UpdateAvailable` — "true if you're able to
    /// update your service software version" — so the member a descriptor reports and
    /// the eligibility [`ServiceSoftware::begin_update`] tests are ONE answer.
    pub fn update_available(&self) -> bool {
        !self.deploying() && self.pending_update.is_none() && self.available_version.is_some()
    }

    /// Start the update a call at `now` asked for, to begin at `starts_at`, or answer
    /// `None` when no update is available — which is the whole eligibility test, so a
    /// caller cannot start one the record does not allow.
    ///
    /// A start at or behind `now` begins the deployment with this call; one still ahead
    /// is recorded as the [`PendingUpdate`] the schedule asked for, which the clock
    /// turns into a deployment when it reaches that instant. Either way the domain keeps
    /// running the release it is on until the deployment lands, and the release it is
    /// moving onto is that deployment's target.
    pub fn begin_update(
        &mut self,
        now: SystemTime,
        starts_at: SystemTime,
    ) -> Option<ServiceSoftwareOptions> {
        if !self.update_available() {
            return None;
        }
        let target = self.available_version.take()?;
        if starts_at <= now {
            self.deployment = Some(SoftwareDeployment {
                target_version: target,
                completes_at: starts_at.checked_add(DOMAIN_WORK).unwrap_or(starts_at),
                rollback: false,
            });
        } else {
            self.pending_update = Some(PendingUpdate {
                target_version: target,
                starts_at,
            });
        }
        Some(self.to_options())
    }

    /// Call off the update this domain has scheduled and not begun, or answer `None`
    /// when there is none to call off — which is the whole eligibility test, so a
    /// caller cannot cancel a deployment the record does not allow cancelling.
    ///
    /// Only an update that has been REQUESTED and has not begun is cancellable (see
    /// [`Self::to_options`]); cancelling it leaves the release it was going to move the
    /// domain onto available to update to again, which is the state the domain was in
    /// before the update was scheduled.
    pub fn cancel_update(&mut self) -> Option<ServiceSoftwareOptions> {
        let pending = self.pending_update.take()?;
        self.available_version = Some(pending.target_version);
        Some(self.to_options())
    }

    /// Whether a service software rollback is available on this domain at `now`: a
    /// deployment has landed leaving an earlier release to return to, its window is
    /// still open, and no deployment is in flight or waiting to begin.
    pub fn rollback_available(&self, now: SystemTime) -> bool {
        !self.deploying()
            && self.pending_update.is_none()
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
            completes_at: now + DOMAIN_WORK,
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
    ///
    /// The last of those is the sentence the service itself answers a domain with no
    /// earlier release to return to — the condition every domain is in until a
    /// deployment has landed on it — so it is reproduced as the service words it,
    /// including the direction it gives a caller that believes a rollback was due.
    pub fn rollback_unavailable(&self) -> RollbackServiceSoftwareOptions {
        let description = if self.deploying() {
            deployment_in_progress_sentence(THIS_DOMAIN)
        } else if self.pending_update.is_some() {
            scheduled_update_sentence(THIS_DOMAIN)
        } else if self.rollback_to.is_some() {
            "The rollback window of this domain's last service software update has closed."
                .to_string()
        } else {
            NO_ROLLBACK_TARGET_DESCRIPTION.to_string()
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
    /// `IN_PROGRESS` while a deployment is being applied, `PENDING_UPDATE` while one
    /// waits for the instant it was asked to start at, `ELIGIBLE` when a release
    /// the domain can take is available, and `COMPLETED` when the domain is settled on
    /// the release it runs.
    pub fn update_status(&self) -> &'static str {
        if self.deploying() {
            DEPLOYMENT_STATUS_IN_PROGRESS
        } else if self.pending_update.is_some() {
            DEPLOYMENT_STATUS_PENDING_UPDATE
        } else if self.available_version.is_some() {
            DEPLOYMENT_STATUS_ELIGIBLE
        } else {
            DEPLOYMENT_STATUS_COMPLETED
        }
    }

    /// "A description of the service software update status" — derived from the record
    /// rather than stored, so it cannot disagree with the status beside it.
    fn description(&self) -> String {
        match (
            &self.deployment,
            &self.pending_update,
            &self.available_version,
        ) {
            (Some(deployment), _, _) if deployment.rollback => format!(
                "Service software rollback to {} is in progress.",
                deployment.target_version
            ),
            (Some(deployment), _, _) => format!(
                "Service software update to {} is in progress.",
                deployment.target_version
            ),
            (None, Some(pending), _) => format!(
                "Service software update to {} is scheduled.",
                pending.target_version
            ),
            (None, None, Some(available)) => {
                format!("Service software update {available} is available.")
            }
            (None, None, None) => no_update_available_sentence(THIS_DOMAIN),
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
                .or_else(|| {
                    self.pending_update
                        .as_ref()
                        .map(|pending| pending.target_version.clone())
                })
                .or_else(|| self.available_version.clone()),
            update_available: Some(self.update_available()),
            // "True if you're able to cancel your service software version update":
            // only a deployment that has been requested and not yet begun can be
            // cancelled, which is an update asked for at an instant still ahead of the
            // call that asked for it.
            cancellable: Some(self.pending_update.is_some()),
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

// ---------------------------------------------------------------------------
// Engine version upgrades
//
// `UpgradeDomain` starts a service-managed engine upgrade — or an upgrade
// eligibility check, which performs none — and `GetUpgradeHistory` reports the runs
// a domain has had. [`UpgradeRun`] below is the ONE record both reach: the run in
// flight and the history entry it becomes are the same record, so the two
// operations cannot disagree about what ran, when it started or how far it got.
// ---------------------------------------------------------------------------

/// How many upgrade runs a domain keeps: `UpgradeHistory` is the "history of the
/// last 10 upgrades and upgrade eligibility checks for an Amazon OpenSearch Service
/// domain", and `GetUpgradeHistory` "retrieves the complete history of the last 10
/// upgrades performed on" one — so an eleventh run drops the oldest.
pub const UPGRADE_HISTORY: usize = 10;

/// `UpgradeStep`'s `PRE_UPGRADE_CHECK`, the first of the "three steps that an upgrade
/// or upgrade eligibility check goes through".
pub const UPGRADE_STEP_PRE_UPGRADE_CHECK: &str = "PRE_UPGRADE_CHECK";

/// `UpgradeStep`'s `SNAPSHOT`; see [`UPGRADE_STEP_PRE_UPGRADE_CHECK`].
pub const UPGRADE_STEP_SNAPSHOT: &str = "SNAPSHOT";

/// `UpgradeStep`'s `UPGRADE`; see [`UPGRADE_STEP_PRE_UPGRADE_CHECK`].
pub const UPGRADE_STEP_UPGRADE: &str = "UPGRADE";

/// The steps an upgrade performs, in the order it performs them.
const UPGRADE_STEPS: [&str; 3] = [
    UPGRADE_STEP_PRE_UPGRADE_CHECK,
    UPGRADE_STEP_SNAPSHOT,
    UPGRADE_STEP_UPGRADE,
];

/// The steps an upgrade eligibility check performs: the check alone, because such a
/// run "does not actually perform the upgrade" — so neither the snapshot an upgrade
/// takes first nor the upgrade itself runs.
const UPGRADE_CHECK_STEPS: [&str; 1] = [UPGRADE_STEP_PRE_UPGRADE_CHECK];

/// `UpgradeStatus`'s `IN_PROGRESS`: the run, or the step, is still being performed.
pub const UPGRADE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `UpgradeStatus`'s `SUCCEEDED`: the run, or the step, finished.
///
/// The other two values the model admits — `SUCCEEDED_WITH_ISSUES` and `FAILED` —
/// report an upgrade that hit trouble on the real cluster; nothing in this emulator
/// can make an upgrade it accepted fail, so no run is ever reported in them.
pub const UPGRADE_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The progress a finished step reports, and the most any step reports.
const STEP_PROGRESS_COMPLETE: u32 = 100;

/// One upgrade, or one upgrade eligibility check, performed on a domain — as this
/// emulator persists it.
///
/// A run is started by `UpgradeDomain` and completes when the span the clock owes it
/// has passed ([`DOMAIN_WORK`], the same span a configuration change takes),
/// materialised by whichever later call reads the record, so nothing sleeps. Its
/// steps are derived from that span rather than stored, so the step a caller polls
/// and the overall status beside it cannot disagree.
#[derive(Debug, Clone)]
pub struct UpgradeRun {
    /// `UpgradeDomainResponse$UpgradeId`: "the unique identifier of the domain
    /// upgrade".
    pub upgrade_id: String,
    /// The engine version the domain ran when this run started.
    pub source_version: String,
    /// The engine version this run moves the domain onto — or, for an eligibility
    /// check, the one it was checked against.
    pub target_version: String,
    /// `UpgradeDomainRequest$PerformCheckOnly`: "when true, indicates that an upgrade
    /// eligibility check needs to be performed. Does not actually perform the
    /// upgrade."
    pub perform_check_only: bool,
    /// The advanced options the upgrade applies to the domain when it lands. An
    /// eligibility check applies none, because it changes nothing.
    pub advanced_options: BTreeMap<String, String>,
    /// `UpgradeHistory$StartTimestamp`: "UTC timestamp at which the upgrade API call
    /// was made".
    pub started_at: SystemTime,
    /// When the clock owes this run completion.
    pub completes_at: SystemTime,
    /// Whether the version this run moves the domain onto has already been
    /// materialised on the domain record, so the landing happens exactly once.
    pub applied: bool,
}

impl UpgradeRun {
    /// The steps this run performs, in order: all three for an upgrade, the check
    /// alone for an eligibility check.
    pub fn steps(&self) -> &'static [&'static str] {
        if self.perform_check_only {
            &UPGRADE_CHECK_STEPS
        } else {
            &UPGRADE_STEPS
        }
    }

    /// Whether this run is still being performed at `now`.
    pub fn in_progress(&self, now: SystemTime) -> bool {
        now < self.completes_at
    }

    /// `UpgradeHistory$UpgradeStatus`: "the current status of the upgrade".
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.in_progress(now) {
            UPGRADE_STATUS_IN_PROGRESS
        } else {
            UPGRADE_STATUS_SUCCEEDED
        }
    }

    /// How long this run has been going at `now`, capped at the span it takes.
    fn elapsed(&self, now: SystemTime) -> u128 {
        let total = self.span();
        now.duration_since(self.started_at)
            .map(|elapsed| elapsed.as_millis().min(total))
            .unwrap_or_default()
    }

    /// The span the clock owes this run, in milliseconds. Never zero, so a caller
    /// reading straight after the call that started the run still sees it in
    /// progress.
    fn span(&self) -> u128 {
        self.completes_at
            .duration_since(self.started_at)
            .unwrap_or(DOMAIN_WORK)
            .as_millis()
            .max(1)
    }

    /// Each step this run has STARTED by `now`, as its place in [`Self::steps`], its
    /// status and its progress percentage.
    ///
    /// The run's span is divided evenly between its steps, so a caller polling a run
    /// in flight sees the steps advance one after another. A step the run has not
    /// reached is not reported at all: `UpgradeHistory$StepsList` is "a list of each
    /// step performed as part of a specific upgrade or upgrade eligibility check".
    fn step_progress(&self, now: SystemTime) -> Vec<(usize, &'static str, u32)> {
        let steps = self.steps();
        let elapsed = self.elapsed(now);
        let span = self.span();
        let count = u128::try_from(steps.len()).unwrap_or(1).max(1);
        (0..steps.len())
            .filter_map(|index| {
                let position = u128::try_from(index).unwrap_or(0);
                let starts_at = span * position / count;
                let ends_at = span * (position + 1) / count;
                if elapsed >= ends_at {
                    return Some((index, UPGRADE_STATUS_SUCCEEDED, STEP_PROGRESS_COMPLETE));
                }
                if elapsed < starts_at {
                    // A step this run has not started performing yet.
                    return None;
                }
                let through = (elapsed - starts_at) * u128::from(STEP_PROGRESS_COMPLETE)
                    / (ends_at - starts_at).max(1);
                Some((
                    index,
                    UPGRADE_STATUS_IN_PROGRESS,
                    u32::try_from(through).unwrap_or(STEP_PROGRESS_COMPLETE),
                ))
            })
            .collect()
    }

    /// The step this run is performing at `now`, and that step's status —
    /// `GetUpgradeStatusResponse`'s `UpgradeStep` and `StepStatus`, "the status of the
    /// current step that an upgrade is on".
    ///
    /// The current step is the LAST one the run has started, which is the last entry of
    /// [`Self::step_progress`] — the same derivation `GetUpgradeHistory` reports every
    /// step from, so the step a caller polls and the step the history lists for the same
    /// run cannot disagree. A finished run is reported on its last step, succeeded.
    pub fn current_step(&self, now: SystemTime) -> Option<(&'static str, &'static str)> {
        let steps = self.steps();
        let (index, status, _) = self.step_progress(now).into_iter().next_back()?;
        Some((*steps.get(index)?, status))
    }

    /// `UpgradeHistory$UpgradeName`: "a string that describes the upgrade".
    ///
    /// The model declares no form for it, so it is derived from the record rather
    /// than stored: which engine versions the run went between, and whether it
    /// performed the upgrade or only checked that it could.
    pub fn upgrade_name(&self) -> String {
        let what = if self.perform_check_only {
            "Upgrade eligibility check"
        } else {
            "Upgrade"
        };
        format!(
            "{what} from {} to {}",
            self.source_version, self.target_version
        )
    }

    /// This run as `UpgradeHistory` — one entry of what `GetUpgradeHistory` answers
    /// with.
    pub fn to_history(&self, now: SystemTime) -> UpgradeHistory {
        let steps = self.steps();
        UpgradeHistory {
            upgrade_name: Some(self.upgrade_name()),
            start_timestamp: Some(datetime(self.started_at)),
            upgrade_status: Some(self.status(now).to_string()),
            steps_list: Some(
                self.step_progress(now)
                    .into_iter()
                    .map(|(index, status, progress)| UpgradeStepItem {
                        upgrade_step: steps.get(index).map(|step| (*step).to_string()),
                        upgrade_step_status: Some(status.to_string()),
                        // "A list of strings containing detailed information about the
                        // errors encountered in a particular step": no step of a run
                        // this emulator performs encounters one.
                        issues: Some(Vec::new()),
                        progress_percent: Some(f64::from(progress)),
                    })
                    .collect(),
            ),
        }
    }
}

/// One configuration change scheduled on a domain, as this emulator persists it.
///
/// A scheduled action is a record with an identity of its own — an action id, a type,
/// a severity and the instant it is to happen — rather than a projection of some
/// other record: `ListScheduledActions` reports it, `DescribeDomainAutoTunes` reports
/// the Auto-Tune ones among them and `UpdateScheduledAction` reschedules it, so this
/// is the ONE copy all of them reach. The operations that SCHEDULE
/// one (`StartServiceSoftwareUpdate` for a service software update, and the Auto-Tune
/// side of `UpdateDomainConfig` for a blue/green enhancement) are still scaffold
/// stubs, so today the only writer is whatever puts a record here directly.
///
/// Its members are kept as the values the model's own enums admit (`ActionType`,
/// `ActionSeverity`, `ScheduledBy`, `ActionStatus`) so the report is the stored
/// record rather than a second derivation of it.
#[derive(Debug, Clone)]
pub struct ScheduledChange {
    /// `ScheduledAction$Id`: "the unique identifier of the scheduled action". Unique
    /// per domain, so it is the key the domain holds the record under and the place a
    /// pagination token names.
    pub id: String,
    /// `ScheduledAction$Type`, one of `ActionType`'s values: which change is
    /// scheduled.
    pub action_type: String,
    /// `ScheduledAction$Severity`, one of `ActionSeverity`'s values.
    pub severity: String,
    /// The instant the change is scheduled to happen, reported as
    /// `ScheduledAction$ScheduledTime`.
    pub scheduled_at: SystemTime,
    /// `ScheduledAction$Description`: "a description of the action to be taken",
    /// absent where the type and severity say it all.
    pub description: Option<String>,
    /// `ScheduledAction$ScheduledBy`, one of `ScheduledBy`'s values: whether a caller
    /// scheduled this change or the service did.
    pub scheduled_by: String,
    /// `ScheduledAction$Status`, one of `ActionStatus`'s values: where the change is
    /// in its own lifecycle.
    pub status: String,
    /// `ScheduledAction$Mandatory`: "whether the action is required or optional".
    pub mandatory: bool,
    /// `ScheduledAction$Cancellable`: "whether or not the scheduled action is
    /// cancellable".
    pub cancellable: bool,
}

impl ScheduledChange {
    /// This record as `ScheduledAction` — the ONE projection of it, so every operation
    /// that reports a scheduled action reports the same values.
    pub fn to_action(&self) -> ScheduledAction {
        ScheduledAction {
            id: self.id.clone(),
            r#type: self.action_type.clone(),
            severity: self.severity.clone(),
            scheduled_time: epoch_millis(self.scheduled_at),
            description: self.description.clone(),
            scheduled_by: Some(self.scheduled_by.clone()),
            status: Some(self.status.clone()),
            mandatory: Some(self.mandatory),
            cancellable: Some(self.cancellable),
        }
    }

    /// Whether this scheduled change is an Auto-Tune optimization of the domain.
    ///
    /// `ScheduledAutoTuneDetails$ActionType` is a `ScheduledAutoTuneActionType`, a
    /// SUBSET of the `ActionType` a scheduled change carries: a change of a type
    /// outside that subset (a service software update) has no `AutoTune` form at all,
    /// so it belongs to the scheduled-action listing and not to the Auto-Tune one.
    /// The subset is the model's own enum, as generated into
    /// [`SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES`], so no second list of it is kept
    /// here.
    pub fn is_auto_tune(&self) -> bool {
        SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES.contains(&self.action_type.as_str())
    }

    /// This record as `AutoTune` — the ONE projection of an Auto-Tune optimization, so
    /// the Auto-Tune report and the scheduled-action listing cannot describe the same
    /// change differently.
    ///
    /// `AutoTuneType` admits only `SCHEDULED_ACTION`, and the detail container carries
    /// the stored instant as `Date` ("the date and time when the Auto-Tune action is
    /// scheduled for the domain"), the stored type and severity, and the stored
    /// description as `Action` ("a description of the Auto-Tune action") — absent for
    /// a change stored without one.
    pub fn to_auto_tune(&self) -> AutoTune {
        AutoTune {
            auto_tune_type: Some(AUTO_TUNE_TYPE_SCHEDULED_ACTION.to_string()),
            auto_tune_details: Some(AutoTuneDetails {
                scheduled_auto_tune_details: Some(ScheduledAutoTuneDetails {
                    date: Some(datetime(self.scheduled_at)),
                    action_type: Some(self.action_type.clone()),
                    action: self.description.clone(),
                    severity: Some(self.severity.clone()),
                }),
            }),
        }
    }

    /// Whether the service has already begun or finished this change, so there is no
    /// longer an instant of it left to move: `ActionStatus`'s
    /// [`ACTION_STATUS_IN_PROGRESS`] and [`ACTION_STATUS_COMPLETED`].
    pub fn under_way(&self) -> bool {
        self.status == ACTION_STATUS_IN_PROGRESS || self.status == ACTION_STATUS_COMPLETED
    }

    /// Re-time this change to `at`, as a caller's reschedule leaves the record.
    ///
    /// The instant is the whole of what the caller chose; the other two members the
    /// move settles follow from the call itself: the change is now one a caller
    /// scheduled rather than one the service picked the time of, and a change that has
    /// been given an instant to happen at is waiting for it.
    pub fn reschedule(&mut self, at: SystemTime) {
        self.scheduled_at = at;
        self.scheduled_by = SCHEDULED_BY_CUSTOMER.to_string();
        self.status = ACTION_STATUS_PENDING_UPDATE.to_string();
    }
}

/// `ActionStatus`'s `PENDING_UPDATE`: the change has an instant to happen at and is
/// waiting for it.
pub const ACTION_STATUS_PENDING_UPDATE: &str = "PENDING_UPDATE";

/// `ActionStatus`'s `IN_PROGRESS`: the change is being applied to the domain.
pub const ACTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ActionStatus`'s `COMPLETED`: the change has been applied to the domain.
pub const ACTION_STATUS_COMPLETED: &str = "COMPLETED";

/// `ScheduledBy`'s `CUSTOMER`: a caller chose when this change happens.
pub const SCHEDULED_BY_CUSTOMER: &str = "CUSTOMER";

/// `ScheduleAt`'s `NOW`: "immediately schedules the update to happen in the current
/// hour if there's capacity available".
pub const SCHEDULE_AT_NOW: &str = "NOW";

/// `ScheduleAt`'s `TIMESTAMP`: "lets you specify a custom date and time to apply the
/// update", which is the one value `DesiredStartTime` belongs with.
pub const SCHEDULE_AT_TIMESTAMP: &str = "TIMESTAMP";

/// `ScheduleAt`'s `OFF_PEAK_WINDOW`: "marks the action to be picked up during an
/// upcoming off-peak window".
pub const SCHEDULE_AT_OFF_PEAK_WINDOW: &str = "OFF_PEAK_WINDOW";

/// How long a day is, for the next opening of a domain's daily off-peak window.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// `at` as a `SystemTime`, from the epoch-millisecond count a `Long` timestamp of
/// this service carries — the inverse of [`epoch_millis`], so a value this service
/// reported and one a caller sent name the same instant.
///
/// The count comes from the CALLER and `Long` bounds nothing, so the arithmetic is
/// checked: a value no instant of this clock can represent answers the far end of the
/// range it ran off rather than unwinding the handler that read it.
pub fn instant_of_epoch_millis(millis: i64) -> SystemTime {
    let offset = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(offset).unwrap_or_else(|| {
            SystemTime::UNIX_EPOCH
                .checked_add(MAX_REPRESENTABLE_OFFSET)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(offset).unwrap_or_else(|| {
            SystemTime::UNIX_EPOCH
                .checked_sub(MAX_REPRESENTABLE_OFFSET)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
    }
}

/// How far from the epoch an instant this service reports can be, for the checked
/// arithmetic of [`instant_of_epoch_millis`]: the largest span every platform's
/// `SystemTime` represents on both sides of the epoch, which is more than four
/// centuries.
const MAX_REPRESENTABLE_OFFSET: Duration = Duration::from_secs(400 * 365 * 24 * 60 * 60);

/// The `ValidationException` payload for a `ScheduleAt` outside the modelled enum:
/// the value asked for and the values that name a schedule.
pub fn schedule_at_outside_the_enum(value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "ScheduleAt must be one of [{}]: {value}",
            SCHEDULE_AT_VALUES.join(", ")
        )),
    }
}

/// Whether the `DesiredStartTime` a request carries belongs with the `ScheduleAt` it
/// names: "only specify this parameter if you set `ScheduleAt` to `TIMESTAMP`", and
/// that value "must also provide a value for `DesiredStartTime`".
///
/// A property of the REQUEST alone, so the operations that schedule a change decide
/// it before resolving anything the request names.
pub fn check_desired_start_time(
    schedule_at: &str,
    desired_start_time: Option<i64>,
) -> Result<(), ValidationException> {
    match (schedule_at, desired_start_time) {
        (SCHEDULE_AT_TIMESTAMP, None) => Err(ValidationException {
            message: Some(format!(
                "DesiredStartTime is required when ScheduleAt is {SCHEDULE_AT_TIMESTAMP}."
            )),
        }),
        (SCHEDULE_AT_TIMESTAMP, Some(_)) => Ok(()),
        (_, None) => Ok(()),
        (_, Some(_)) => Err(ValidationException {
            message: Some(format!(
                "DesiredStartTime must only be specified when ScheduleAt is \
                 {SCHEDULE_AT_TIMESTAMP}: {schedule_at}."
            )),
        }),
    }
}

/// The instant a reschedule puts a change at, for a request whose `ScheduleAt` and
/// `DesiredStartTime` [`check_desired_start_time`] has already admitted.
///
/// Each `ScheduleAt` value names its own instant: `NOW` the moment of the call,
/// `TIMESTAMP` the one the caller gave, and `OFF_PEAK_WINDOW` the next opening of
/// the window `domain` carries ([`Domain::next_off_peak_window_start`]).
pub fn rescheduled_instant(
    domain: &Domain,
    schedule_at: &str,
    desired_start_time: Option<i64>,
    now: SystemTime,
) -> SystemTime {
    match schedule_at {
        // `NOW` schedules "the update to happen in the current hour", which is the
        // instant of the call itself.
        SCHEDULE_AT_NOW => now,
        SCHEDULE_AT_TIMESTAMP => desired_start_time.map_or(now, instant_of_epoch_millis),
        SCHEDULE_AT_OFF_PEAK_WINDOW => domain.next_off_peak_window_start(now),
        // `ScheduleAt` is an enum of exactly those three values and the boundary
        // validator refuses anything else before a handler runs, so this arm stands for
        // nothing a request can carry.
        _ => now,
    }
}

/// The `ValidationException` payload for a request whose `ActionType` is not the type
/// of the change its `ActionID` names: "the type of action to reschedule … to
/// retrieve this value, send a `ListScheduledActions` request", so the two members
/// must name ONE change.
pub fn scheduled_action_type_mismatch(
    action_id: &str,
    requested: &str,
    stored: &str,
) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Scheduled action {action_id} is of type {stored}, not {requested}."
        )),
    }
}

/// The `ResourceNotFoundException` payload for an `ActionID` that names no scheduled
/// change of a domain the store DOES hold, naming both so a caller tells it apart
/// from [`domain_not_found`].
pub fn scheduled_action_not_found(
    action_id: &str,
    domain_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Scheduled action {action_id} doesn't exist for domain {domain_name}."
        )),
    }
}

/// The `ConflictException` payload for re-timing a change the service has already
/// begun or finished: there is no instant of it left to move, and the refusal names
/// the status that says so.
pub fn scheduled_action_under_way(action_id: &str, status: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Scheduled action {action_id} is {status} and can no longer be rescheduled."
        )),
    }
}

/// `AutoTuneType`'s only value: every Auto-Tune optimization this service reports is
/// a scheduled action.
pub const AUTO_TUNE_TYPE_SCHEDULED_ACTION: &str = "SCHEDULED_ACTION";

/// `InsightFieldType`'s `text` value — "possible values are `text` and `metric`" —
/// the type of a field that carries a text description rather than metric data. One
/// of [`crate::ops::types::INSIGHT_FIELD_TYPE_VALUES`].
pub const INSIGHT_FIELD_TYPE_TEXT: &str = "text";

/// One field of an insight's DETAILED view, as this emulator persists it.
///
/// A detail read "returns detailed fields associated with the specified insight, such
/// as text descriptions and metric data", each field a name, a type out of
/// `InsightFieldType` (`text` or `metric`) and a value — so this is the ONE copy of a
/// field, held on the insight it details ([`InsightRecord::details`]) in the order a
/// detail read reports it.
///
/// A field's content is stored once per rendering the service raises it in: [`value`]
/// is the plain one and [`html_value`] the SAME content with the markup the service
/// renders it with, for a field that has such a rendering. The request's
/// `ShowHtmlContent` picks between them ([`InsightDetailField::to_insight_field`]), so
/// neither is derived from the other at read time.
///
/// [`value`]: InsightDetailField::value
/// [`html_value`]: InsightDetailField::html_value
#[derive(Debug, Clone)]
pub struct InsightDetailField {
    /// `InsightField$Name`: "the name of the insight field".
    pub name: String,
    /// `InsightField$Type`, one of `InsightFieldType`'s values: whether this field
    /// carries a text description or metric data.
    pub field_type: String,
    /// `InsightField$Value` with no markup in it — what a caller that did not ask for
    /// HTML content is answered with.
    pub value: String,
    /// The same content as the service renders it WITH markup, for a field that has
    /// such a rendering; `None` where it has none and the plain value is the whole of
    /// what there is to report.
    pub html_value: Option<String>,
}

impl InsightDetailField {
    /// This field as `InsightField` — the ONE projection of it, so every operation
    /// that reports a detail field reports the same values.
    ///
    /// `show_html` is the request's `ShowHtmlContent` — "specifies whether to show
    /// response with HTML content in response or not" — so it selects the marked-up
    /// rendering where this field has one, and changes nothing where it has none.
    pub fn to_insight_field(&self, show_html: bool) -> InsightField {
        let value = match (show_html, self.html_value.as_deref()) {
            (true, Some(html)) => html.to_string(),
            _ => self.value.clone(),
        };
        InsightField {
            name: self.name.clone(),
            r#type: self.field_type.clone(),
            value,
        }
    }
}

/// The feedback a caller has submitted on one insight, as this emulator persists it.
///
/// A submission "allows users to provide a thumbs up or thumbs down rating and
/// optional text feedback for a specific insight", so a record is that rating, the
/// text that came with it where the caller wrote any, and the instant it was
/// submitted. It is held on the insight it rates ([`InsightRecord::feedback`]), so it
/// goes when that insight goes.
///
/// One caller holds ONE rating of an insight — "thumbs up or thumbs down" is a single
/// verdict, not a tally — so a later submission about the same insight REPLACES the
/// rating an earlier one left rather than accumulating beside it.
#[derive(Debug, Clone)]
pub struct InsightFeedbackRecord {
    /// `InsightFeedbackRequest$Thumbs`, one of `InsightFeedbackThumbs`' values: the
    /// thumbs up or thumbs down rating the caller gave the insight.
    pub thumbs: String,
    /// `InsightFeedbackRequest$FeedbackText`: "optional text feedback providing
    /// additional details about the insight", `None` where the submission carried
    /// none.
    pub feedback_text: Option<String>,
    /// The instant this rating was submitted — the submission this record is of.
    pub submitted_at: SystemTime,
}

/// One insight, as this emulator persists it.
///
/// An insight is "a notification about a domain event or recommendation that helps
/// you optimize your Amazon OpenSearch Service domain": a record with an identity of
/// its own — a `GUID`, a display name, a type, a priority, a status and the instants
/// it was created and last changed at — so this is the ONE copy `ListInsights`
/// reports and the one `DescribeInsightDetails` and `InsightFeedback` address by id.
/// The service itself raises an insight; no operation of this emulator does yet, so
/// today the only writer is whatever puts a record here directly.
///
/// Its members are kept as the values the model's own enums admit (`InsightType`,
/// `InsightPriorityLevel`, `InsightStatus`) so a listing reports the stored record
/// rather than a second derivation of it.
#[derive(Debug, Clone)]
pub struct InsightRecord {
    /// `Insight$InsightId`: "the unique identifier of the insight", a [`new_guid`].
    /// Unique across the account's insights, so it is the key the set holds the
    /// record under and the place a pagination token names.
    pub insight_id: String,
    /// `Insight$DisplayName`: "the display name of the insight".
    pub display_name: String,
    /// `Insight$Type`, one of `InsightType`'s values: whether the insight reports a
    /// domain event or a recommendation.
    pub insight_type: String,
    /// `Insight$Priority`, one of `InsightPriorityLevel`'s values.
    pub priority: String,
    /// `Insight$Status`, one of `InsightStatus`'s values: where the insight is in its
    /// own lifecycle.
    pub status: String,
    /// The instant the insight was raised, reported as `Insight$CreationTime` — and
    /// the instant a listing's time range and sort order are measured on.
    pub created_at: SystemTime,
    /// The instant the insight last changed, reported as `Insight$UpdateTime`.
    pub updated_at: SystemTime,
    /// `Insight$IsExperimental`: "indicates whether the insight is experimental".
    pub is_experimental: bool,
    /// The fields of this insight's DETAILED view, in the order a detail read reports
    /// them — the text descriptions and metric data a detail read answers with. Held
    /// on the insight itself, because a detail field details THAT insight and goes
    /// when it goes; empty for an insight the service has raised no detail for.
    pub details: Vec<InsightDetailField>,
    /// The feedback submitted on THIS insight, or `None` while no caller has rated it.
    /// Held on the insight itself, because feedback rates THAT insight and goes when
    /// it goes. See [`InsightFeedbackRecord`].
    pub feedback: Option<InsightFeedbackRecord>,
}

impl InsightRecord {
    /// This record as `Insight` — the ONE projection of it, so every operation that
    /// reports an insight reports the same values.
    pub fn to_insight(&self) -> Insight {
        Insight {
            insight_id: Some(self.insight_id.clone()),
            display_name: Some(self.display_name.clone()),
            r#type: Some(self.insight_type.clone()),
            priority: Some(self.priority.clone()),
            status: Some(self.status.clone()),
            creation_time: Some(datetime(self.created_at)),
            update_time: Some(datetime(self.updated_at)),
            is_experimental: Some(self.is_experimental),
        }
    }
}

/// The entity type values `InsightEntityType` admits: an insight is held either for
/// one domain or for the account itself.
pub const INSIGHT_ENTITY_ACCOUNT: &str = "Account";
/// See [`INSIGHT_ENTITY_ACCOUNT`].
pub const INSIGHT_ENTITY_DOMAIN_NAME: &str = "DomainName";

/// The values `InsightFeedbackThumbs` admits: "the thumbs up or thumbs down feedback
/// for an insight. Possible values are `Up` and `Down`". One of
/// [`crate::ops::types::INSIGHT_FEEDBACK_THUMBS_VALUES`].
pub const INSIGHT_FEEDBACK_THUMBS_UP: &str = "Up";
/// See [`INSIGHT_FEEDBACK_THUMBS_UP`].
pub const INSIGHT_FEEDBACK_THUMBS_DOWN: &str = "Down";

/// Why the entity a request names has no insights to report, as the modelled
/// exception payload the refusal is reported through.
///
/// Each operation declares its OWN error enum, so this names the payload and the
/// handler lifts it into its own channel: resolving an `InsightEntity` is ONE
/// decision even though the error channels of the operations that make it are not.
#[derive(Debug)]
pub enum InsightEntityRefusal {
    /// The entity names a resource this account does not hold.
    NotFound(ResourceNotFoundException),
    /// The entity itself is not one this service can resolve.
    Validation(ValidationException),
}

/// `at` as the epoch-millisecond count a `Long` timestamp of this service carries —
/// the unit the model states for its epoch timestamps (`InsightTimeRange`'s members
/// are "epoch millisecond timestamps"), negative for an instant before the epoch.
pub fn epoch_millis(at: SystemTime) -> i64 {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis())
            .map(|millis| -millis)
            .unwrap_or(i64::MIN),
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
    ///
    /// This is the PUBLIC host, which only a domain provisioned OUTSIDE a VPC is
    /// reachable on; a VPC-access domain is reached on the hosts
    /// [`Self::vpc_access_endpoints`] publishes instead.
    pub endpoint: String,
    /// The opaque id the hosts of a VPC-access domain's endpoints carry, minted once
    /// for the domain when it comes to hold `VPCOptions` and `None` while it holds
    /// none — so the hosts such a domain publishes are stable across reads and no two
    /// domains share one. Minted by
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::provision_vpc_access_host`],
    /// the ONE place that decides a domain needs one.
    ///
    /// Distinct from a service-managed [`VpcEndpointRecord`], which is a resource of
    /// its own that `CreateVpcEndpoint` mints to reach this domain from ANOTHER VPC;
    /// this is the domain's own address inside the VPC it was provisioned in.
    pub vpc_access_suffix: Option<String>,
    /// `DomainStatus$Created`.
    pub created: bool,
    /// `DomainStatus$Deleted`. Only ever true on the record `DeleteDomain` returns:
    /// the store no longer holds a deleted domain.
    pub deleted: bool,
    /// `DomainStatus$Processing`: a configuration change is in flight.
    pub processing: bool,
    /// One of `DomainProcessingStatusType`'s values.
    pub processing_status: String,
    /// When the clock owes this domain a settled configuration; see [`DOMAIN_WORK`].
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
    /// The ARN of the IAM principal `MasterUserOptions$MasterUserARN` named as this
    /// domain's fine-grained-access-control master user, and the name
    /// `MasterUserOptions$MasterUserName` named for an internal one.
    ///
    /// Held because the master user is WHO the cluster grants everything to
    /// ([`fgac::is_master`]) — the decision every request to a domain with fine-grained
    /// access control switched on is taken by — and it is reachable from nowhere else: the
    /// `AdvancedSecurityOptions` shape every read path reports declares no member for
    /// either, so neither is ever reported back, and the master user's PASSWORD is not
    /// held at all.
    pub master_user_arn: Option<String>,
    /// See [`Self::master_user_arn`].
    pub master_user_name: Option<String>,
    /// The roles this domain's security configuration holds, keyed by their name — unique
    /// in the configuration, so the name IS the key. Written and reported by the domain's
    /// own security API ([`crate::service::domain_rest`]).
    pub security_roles: BTreeMap<String, fgac::SecurityRole>,
    /// The role MAPPINGS of that configuration, keyed by the role each binds principals
    /// to — one mapping per role, so the role's name IS the key. What decides whether a
    /// principal that is not the master user may reach this domain at all
    /// ([`fgac::mapped_roles`]).
    pub security_role_mappings: BTreeMap<String, fgac::SecurityRoleMapping>,
    /// The index templates this domain holds, keyed by their name — unique on the domain,
    /// so the name IS the key. See [`IndexTemplate`].
    pub index_templates: BTreeMap<String, IndexTemplate>,
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
    /// The resource tags attached to this domain: the ONE copy `CreateDomain`'s
    /// `TagList`, `AddTags`, `RemoveTags` and `ListTags` all write and read, so no
    /// two of them can report the same domain's tags differently. See [`TagSet`].
    pub tags: TagSet,
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
    /// Every principal allowed to reach this domain through an interface VPC
    /// endpoint, keyed by the principal itself — an account id or a service
    /// principal, which is what identifies it, so one principal holds at most one
    /// authorization however many times it is authorized. Bounded by
    /// [`VPC_ENDPOINT_ACCESS_QUOTA`].
    ///
    /// `AuthorizeVpcEndpointAccess` mints an entry, `ListVpcEndpointAccess` reports
    /// the set and `RevokeVpcEndpointAccess` removes one — all from THIS map, so the
    /// grant's own answer and a later listing cannot disagree. Held on the domain
    /// rather than beside it, because an authorization is access TO a domain: the
    /// record goes when `DeleteDomain` releases the domain it belongs to.
    ///
    /// Iterating a `BTreeMap` is order-stable, which is what lets a paged listing
    /// resume where the previous page stopped without repeating or skipping a
    /// principal.
    pub vpc_endpoint_access: BTreeMap<String, AuthorizedPrincipal>,
    /// This domain's OpenSearch indexes, keyed by their `IndexName` — unique per
    /// domain, so the name IS the key. `CreateIndex` mints one and `GetIndex`
    /// reports it, from THIS one record, and deleting the domain releases every
    /// index on it. Bounded by [`INDEX_QUOTA`].
    pub indexes: BTreeMap<String, Index>,
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
    /// The verbose dry runs started against this domain, oldest first, bounded by
    /// [`DRY_RUN_HISTORY`]: an `UpdateDomainConfig` asking for one mints the record
    /// and `DescribeDryRunProgress` reports it, from THIS one record. A FIFO, because
    /// the cap drops the oldest run and the newest is the one that operation reports
    /// when the request names no `DryRunId`.
    pub dry_runs: VecDeque<DryRun>,
    /// The node maintenance actions started on this domain, oldest first, bounded by
    /// [`MAINTENANCE_HISTORY`]: `StartDomainMaintenance` mints one and
    /// `GetDomainMaintenanceStatus` reports it, from THIS one record. A FIFO, because
    /// the cap drops the oldest action.
    pub maintenances: VecDeque<Maintenance>,
    /// The engine version upgrades and upgrade eligibility checks performed on this
    /// domain, oldest first, bounded by [`UPGRADE_HISTORY`]: a FIFO, because the cap
    /// drops the oldest run and the newest is the one still in flight. See
    /// [`UpgradeRun`].
    pub upgrades: VecDeque<UpgradeRun>,
    /// The configuration changes scheduled on this domain, keyed by their action id
    /// — unique per domain, so the id IS the key. See [`ScheduledChange`].
    pub scheduled_actions: BTreeMap<String, ScheduledChange>,
    /// The insights this domain holds, keyed by their `InsightId` — unique across
    /// the account's insights, so the id IS the key. See [`InsightRecord`].
    ///
    /// Held on the domain rather than beside it, because an insight of a
    /// `DomainName` entity is an insight ABOUT a domain: the record goes when
    /// `DeleteDomain` releases the domain it belongs to. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads report the same insights in the
    /// same order.
    pub insights: BTreeMap<String, InsightRecord>,
}

impl Domain {
    /// The schema an index named `index_name` is CREATED with on this domain when the
    /// create itself names none: the schema of the matching index template, the
    /// highest-priority one where several match, and `None` where no template matches.
    ///
    /// Of two templates of equal priority matching one name the earlier name in
    /// configuration order wins, so the answer does not depend on iteration luck.
    #[must_use]
    pub fn templated_schema(&self, index_name: &str) -> Option<&::aws_smithy_types::Document> {
        self.index_templates
            .values()
            .filter(|template| {
                template.index_patterns.iter().any(|pattern| {
                    ::service_common::iam_authorization::wildcard_match(pattern, index_name, true)
                })
            })
            .fold(None, |best: Option<&IndexTemplate>, template| match best {
                // A template of EQUAL priority does not displace one already chosen, and
                // a `BTreeMap` iterates by name, so the first matching name wins.
                Some(best) if best.priority >= template.priority => Some(best),
                _ => Some(template),
            })
            .map(|template| &template.schema)
    }

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
        self.processing_until = Some(now + DOMAIN_WORK);
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
            // Where this change can be gone back from, the call that started it
            // records the configuration it started from ([`Self::record_revert`]).
            revert_to: None,
            cancelled: false,
        });
        // The history is bounded: past its cap the oldest change is dropped, so a
        // domain updated forever holds a fixed number of records.
        while self.changes.len() > CHANGE_HISTORY {
            self.changes.pop_front();
        }
    }

    /// Record the configuration the change this domain has just started was applied
    /// ON TOP of, so cancelling that change puts it back.
    ///
    /// Called by the update path right after [`Self::begin_change`]: the snapshot is
    /// of the stored record as it was BEFORE the merge, which is the configuration
    /// the change moves the domain away from.
    pub fn record_revert(&mut self, options: DomainOptions) {
        if let Some(change) = self.changes.back_mut() {
            change.revert_to = Some(options);
        }
    }

    /// The configuration changes of this domain a cancel would cancel — the ones
    /// still in flight that can be gone back from — oldest first.
    pub fn cancellable_changes(&self) -> Vec<&ConfigChange> {
        self.changes
            .iter()
            .filter(|change| change.cancellable())
            .collect()
    }

    /// Cancel every configuration change of this domain that is still in flight and
    /// can be gone back from: put back the configuration the EARLIEST of them
    /// started from — which is what the whole chain of them moved the domain away
    /// from — and leave each change in the history marked cancelled, so a change a
    /// caller already read stays addressable and reports what became of it.
    ///
    /// The domain stops `Processing` as soon as no change of it is in flight any
    /// more: there is then nothing left for the clock to make due.
    pub fn cancel_changes(&mut self, now: SystemTime) {
        let revert = self
            .changes
            .iter()
            .find(|change| change.cancellable())
            .and_then(|change| change.revert_to.clone());
        for change in &mut self.changes {
            if !change.cancellable() {
                continue;
            }
            change.completes_at = None;
            change.cancelled = true;
            change.last_updated_at = now;
            change.revert_to = None;
        }
        if let Some(options) = revert {
            options.restore(self);
        }
        self.updated_at = now;
        if self.changes.iter().all(|change| !change.in_progress()) {
            self.processing = false;
            self.processing_status = PROCESSING_STATUS_ACTIVE.to_string();
            self.processing_until = None;
        }
    }

    /// Start the node maintenance action a call at `now` asked for, and answer the
    /// `MaintenanceId` the record it created is addressed by.
    ///
    /// The action is `PENDING` when it is started and the clock owes it the rest of
    /// its run ([`Maintenance::settle`]), so a caller reading straight after the
    /// call still observes it waiting.
    pub fn begin_maintenance(
        &mut self,
        action: &str,
        node_id: &str,
        now: SystemTime,
    ) -> String {
        let maintenance_id = new_maintenance_id();
        self.maintenances.push_back(Maintenance {
            maintenance_id: maintenance_id.clone(),
            action: action.to_string(),
            node_id: node_id.to_string(),
            status: MAINTENANCE_STATUS_PENDING.to_string(),
            created_at: now,
            updated_at: now,
            starts_at: Some(now + MAINTENANCE_STEP),
            completes_at: Some(now + MAINTENANCE_STEP + MAINTENANCE_STEP),
        });
        // The history is bounded the same way the configuration changes are: past
        // its cap the oldest action is dropped.
        while self.maintenances.len() > MAINTENANCE_HISTORY {
            self.maintenances.pop_front();
        }
        maintenance_id
    }

    /// The maintenance action `maintenance_id` names, or `None` when this domain's
    /// history holds no such action.
    pub fn maintenance(&self, maintenance_id: &str) -> Option<&Maintenance> {
        self.maintenances
            .iter()
            .find(|maintenance| maintenance.maintenance_id == maintenance_id)
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

    /// Record the verbose dry run a call at `now` asked for, and answer it.
    ///
    /// `config` is the `DomainStatus` of the configuration the change would leave the
    /// domain in and `deployment_type` the deployment kind it would need, both
    /// computed by the update path's own analysis of a COPY of this record — nothing
    /// about the domain itself moves, because a dry run applies nothing. The run is
    /// [`DRY_RUN_STATUS_PENDING`] and the clock owes it its conclusion
    /// ([`DryRun::settle`]), so a caller reading straight after the call still
    /// observes the analysis in flight.
    pub fn begin_dry_run(
        &mut self,
        config: DomainStatus,
        deployment_type: &str,
        accepted_warnings: Vec<String>,
        now: SystemTime,
    ) -> DryRun {
        let run = DryRun {
            dry_run_id: new_dry_run_id(),
            started_at: now,
            last_updated_at: now,
            completes_at: Some(now + PROVISIONING),
            config,
            deployment_type: deployment_type.to_string(),
            accepted_warnings,
        };
        self.dry_runs.push_back(run.clone());
        // The history is bounded the same way the configuration changes are: past
        // its cap the oldest run is dropped.
        while self.dry_runs.len() > DRY_RUN_HISTORY {
            self.dry_runs.pop_front();
        }
        run
    }

    /// The most recent dry run, which `DescribeDryRunProgress` reports when the
    /// request names no `DryRunId`.
    pub fn latest_dry_run(&self) -> Option<&DryRun> {
        self.dry_runs.back()
    }

    /// The dry run `dry_run_id` names, or `None` when this domain's history holds no
    /// such run.
    pub fn dry_run(&self, dry_run_id: &str) -> Option<&DryRun> {
        self.dry_runs
            .iter()
            .find(|run| run.dry_run_id == dry_run_id)
    }

    /// Start the engine version upgrade — or the upgrade eligibility check — a call at
    /// `now` asked for, and answer the record it created.
    ///
    /// An upgrade is a configuration change of the domain like any other: the domain
    /// goes `UpgradingEngineVersion` and its `EngineVersion` option reports
    /// `Processing` until the clock makes the run due, and only THEN does the domain
    /// report the version it was upgraded to ([`Self::settle`]). A check performs no
    /// upgrade, so it starts no change and moves nothing about the domain — including
    /// its history: see [`Self::eligibility_check`].
    pub fn begin_upgrade(
        &mut self,
        target_version: &str,
        perform_check_only: bool,
        advanced_options: BTreeMap<String, String>,
        now: SystemTime,
    ) -> UpgradeRun {
        // An eligibility check performs nothing, so performing it again records
        // nothing: the check the domain already holds for the version it runs IS the
        // check, whatever target a later call names.
        if perform_check_only {
            if let Some(existing) = self.eligibility_check() {
                return existing.clone();
            }
        }
        let run = UpgradeRun {
            upgrade_id: new_upgrade_id(),
            source_version: self.engine_version.clone(),
            target_version: target_version.to_string(),
            perform_check_only,
            advanced_options,
            started_at: now,
            completes_at: now + DOMAIN_WORK,
            applied: false,
        };
        if !perform_check_only {
            let mut properties = vec![OPT_ENGINE_VERSION];
            if !run.advanced_options.is_empty() {
                // "The advanced options configuration for the domain": an upgrade that
                // carries them changes those too, so they are part of the change it
                // starts.
                properties.push(OPT_ADVANCED_OPTIONS);
            }
            for option in &properties {
                self.touch(option, now);
            }
            self.begin_change(
                PROCESSING_STATUS_UPGRADING_ENGINE_VERSION,
                &properties,
                now,
            );
        }
        self.upgrades.push_back(run.clone());
        // The history is bounded: past its cap the oldest run is dropped, so a domain
        // upgraded forever holds a fixed number of records.
        while self.upgrades.len() > UPGRADE_HISTORY {
            self.upgrades.pop_front();
        }
        run
    }

    /// The upgrade eligibility check this domain already holds for the engine version
    /// it runs now, if any.
    ///
    /// A check "does not actually perform the upgrade", so it is a statement about the
    /// version the domain is ON rather than a run of its own: a domain holds at most
    /// one such record per engine version, and a later check from that same version
    /// reports it rather than recording a second. The oldest matching record is the
    /// one, so the record a repeat check reports keeps the identity, name and start
    /// instant the first check gave it.
    pub fn eligibility_check(&self) -> Option<&UpgradeRun> {
        self.upgrades
            .iter()
            .find(|run| run.perform_check_only && run.source_version == self.engine_version)
    }

    /// Whether an engine version upgrade is being performed on this domain right now
    /// — `DomainStatus$UpgradeProcessing`.
    ///
    /// Read from the record AFTER [`Self::settle`] has materialised whatever the clock
    /// owed it, so a run whose span has passed is no longer in flight. An eligibility
    /// check performs no upgrade, so one in progress is not an upgrade in process.
    pub fn upgrade_in_flight(&self) -> bool {
        self.upgrades
            .back()
            .is_some_and(|run| !run.perform_check_only && !run.applied)
    }

    /// The most recent upgrade or upgrade eligibility check, which `GetUpgradeStatus`
    /// reports on.
    pub fn latest_upgrade(&self) -> Option<&UpgradeRun> {
        self.upgrades.back()
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
        // A verbose dry run is owed its conclusion by the same clock, and on the same
        // span: it analyses a change rather than applying one, so it is not one of the
        // domain's configuration changes.
        for run in &mut self.dry_runs {
            run.settle(now);
        }
        // A node maintenance action is owed its run by the same clock, on its own
        // spans: it is not one of the domain's configuration changes.
        for maintenance in &mut self.maintenances {
            maintenance.settle(now);
        }
        // A service software deployment is owed its completion by the same clock, and
        // on its own span: it is not one of the domain's configuration changes.
        self.service_software.settle(now);
        // An association of a package with this domain is owed its outcome by that
        // same clock, and likewise on its own span.
        self.settle_package_associations(now);
        // An engine version upgrade is owed its completion by the same clock, on the
        // span the call that started it recorded: once that has passed the domain runs
        // the version it was upgraded to, and the advanced options that upgrade carried
        // are the domain's. It lands exactly once, however many callers read the
        // record afterwards.
        let landed = self
            .upgrades
            .back_mut()
            .filter(|run| !run.perform_check_only && !run.applied && now >= run.completes_at)
            .map(|run| {
                run.applied = true;
                (run.target_version.clone(), run.advanced_options.clone())
            });
        if let Some((target_version, advanced_options)) = landed {
            self.engine_version = target_version;
            self.advanced_options.extend(advanced_options);
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

    /// Materialise what the clock owes this domain's package associations: a status
    /// move that has fallen due lands, and a removal that has fallen due either takes
    /// the record off the domain or FAILS, leaving it there.
    ///
    /// Which of the two a due removal does is decided here, at the instant it happens,
    /// from the indexes the domain holds then: taking a package off means taking its
    /// content off the cluster nodes, so a domain holding an index whose analysis
    /// settings resolve a file that package provides cannot let go of it. Such a
    /// removal leaves the association in [`DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED`]
    /// carrying what there is to say about it ([`PACKAGE_IN_USE`]), so the index keeps
    /// reading the file it names and a caller is told to remove the dependency first.
    /// Deciding it HERE rather than at the call is what makes the dangling state
    /// unreachable whichever order the two calls arrive in: an index created while the
    /// removal is in flight still depends on the package when the removal falls due.
    ///
    /// ONE method rather than a loop at each call site, because the parts belong
    /// together: a record whose removal has happened must not be reported by ANY read of
    /// the set, and `DomainPackageStatus` has no value for a package that is no longer
    /// on the domain. [`Self::settle`] calls it, and so does every operation that
    /// reports associations.
    ///
    /// This is the only write such a report makes, and it writes only what was already
    /// owed at the same instant had nobody called — which is why a failed removal is
    /// stamped with the instant it fell DUE rather than with the instant of the call
    /// that happened to observe it.
    pub fn settle_package_associations(&mut self, now: SystemTime) {
        for association in self.package_associations.values_mut() {
            association.settle(now);
        }
        let due: Vec<(String, SystemTime)> = self
            .package_associations
            .iter()
            .filter_map(|(package_id, association)| {
                let due = association.removal_due.filter(|due| now >= *due)?;
                Some((package_id.clone(), due))
            })
            .collect();
        for (package_id, due) in due {
            if self.index_resolving(&package_id).is_some() {
                if let Some(association) = self.package_associations.get_mut(&package_id) {
                    association.status = DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED.to_string();
                    association.removal_due = None;
                    association.last_updated = due;
                    association.error_details = Some(ErrorDetails {
                        error_type: Some(DOMAIN_PACKAGE_ERROR_TYPE_IN_USE.to_string()),
                        error_message: Some(PACKAGE_IN_USE.to_string()),
                    });
                }
            } else {
                self.package_associations.remove(&package_id);
            }
        }
    }

    /// The index of this domain whose analysis settings resolve a file the package
    /// `package_id` names provides, if the domain holds one.
    ///
    /// What makes a package IN USE on the domain: its content is read by an index that
    /// is there, so taking the package off would leave that index's analyzers pointing
    /// at nothing. Iterated in name order, so the same set always answers the same
    /// index.
    ///
    /// Reads ONLY, and reads the schema each index was created or last updated with —
    /// which is the schema the cluster resolves.
    pub fn index_resolving(&self, package_id: &str) -> Option<&Index> {
        self.indexes.values().find(|index| {
            package_references(&index.schema)
                .iter()
                .any(|reference| reference.names_package(package_id))
        })
    }

    /// The first package file `schema`'s analysis settings name that this domain cannot
    /// resolve, or `None` when it can resolve every one of them.
    ///
    /// An index is served by the cluster, so a file its analysis settings name has to
    /// BE there to be read: the package that provides it must be in place on the domain
    /// ([`PackageAssociation::in_place`]). A path naming a package the domain does not
    /// hold, or holds only an association that has not landed yet, resolves to nothing.
    ///
    /// Reads ONLY, and is read AFTER [`Self::settle_package_associations`], so an
    /// association whose removal has fallen due is already off the domain.
    pub fn unresolved_package_reference(
        &self,
        schema: &::aws_smithy_types::Document,
    ) -> Option<PackageReference> {
        package_references(schema).into_iter().find(|reference| {
            !self
                .package_associations
                .get(&reference.package_id)
                .is_some_and(PackageAssociation::in_place)
        })
    }

    /// The next instant this domain's off-peak window opens after `now` — where a
    /// change marked "to be picked up during an upcoming off-peak window" lands.
    ///
    /// The window is "a custom 10-hour, low-traffic window during which OpenSearch
    /// Service can perform mandatory configuration changes on the domain", opening at
    /// the `WindowStartTime` the domain carries, in UTC. `Hours` and `Minutes` are
    /// `@default(0)`, so a domain that configured no custom start has its window open
    /// at the start of the UTC day. The instant answered is strictly after `now`,
    /// because a window that has already opened today is not an UPCOMING one.
    ///
    /// Every domain HAS such a window: `OffPeakWindowOptions$Enabled` "is only
    /// available when modifying a domain created prior to February 16, 2023, not when
    /// creating a new domain. All domains created after that date have an off-peak
    /// window by default" — so the toggle says nothing about a domain this emulator
    /// mints, and only the start time is read here.
    pub fn next_off_peak_window_start(&self, now: SystemTime) -> SystemTime {
        let start = self
            .off_peak_window_options
            .as_ref()
            .and_then(|options| options.off_peak_window.as_ref())
            .and_then(|window| window.window_start_time.as_ref());
        // `Hours` is `@range(0..23)` and `Minutes` `@range(0..59)`, refused outside
        // those bounds before any handler stored them; clamping is what keeps the
        // offset inside one day whatever the record holds.
        let hours = start.map_or(0, |time| time.hours).clamp(0, 23);
        let minutes = start.map_or(0, |time| time.minutes).clamp(0, 59);
        let into_day = Duration::from_secs(hours.unsigned_abs() * 3600 + minutes.unsigned_abs() * 60);
        let since_epoch = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let day_started = SystemTime::UNIX_EPOCH
            + Duration::from_secs(since_epoch - since_epoch % DAY.as_secs());
        let today = day_started + into_day;
        if today > now {
            today
        } else {
            today + DAY
        }
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

    /// The hosts a VPC-access domain is reachable on, as `DomainStatus$Endpoints`
    /// reports them — or `None` for a domain reached on a public endpoint, which
    /// publishes no such map.
    ///
    /// `Endpoints` is "the key-value pair that exists if the OpenSearch Service domain
    /// uses VPC endpoints": the IPv4 host under `vpc`, and beside it the dual-stack
    /// host under `vpcv2` for a domain whose `IPAddressType` is `dualstack`, which is
    /// the setting that gives a domain a dual-stack host at all.
    ///
    /// Ordered by key, so two reads of one domain publish the hosts in the same order:
    /// what a caller sees of a stored record cannot depend on how the map was built.
    pub fn vpc_access_endpoints(&self) -> Option<BTreeMap<String, String>> {
        // A domain is VPC-access exactly when it holds `VPCOptions`; the suffix is the
        // host id minted for it when it came to hold them.
        let suffix = match (self.vpc_options.as_ref(), self.vpc_access_suffix.as_deref()) {
            (Some(_), Some(suffix)) => suffix,
            _ => return None,
        };
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            ENDPOINTS_KEY_VPC.to_string(),
            vpc_access_endpoint(suffix, &self.region),
        );
        if self.ip_address_type.as_deref() == Some(IP_ADDRESS_TYPE_DUALSTACK) {
            endpoints.insert(
                ENDPOINTS_KEY_VPC_DUALSTACK.to_string(),
                vpc_access_endpoint_v2(suffix, &self.region),
            );
        }
        Some(endpoints)
    }

    /// The DUAL-STACK host a domain reached on a PUBLIC endpoint is also reachable on,
    /// as `DomainStatus$EndpointV2` reports it — or `None` for a domain that has no
    /// version 2 endpoint.
    ///
    /// "If `IPAddressType` [is] set to `dualstack`, a version 2 domain endpoint is
    /// provisioned. This endpoint functions like a normal endpoint, except that it works
    /// with both IPv4 and IPv6 IP addresses": so the domains that have one are exactly
    /// the `dualstack` ones, and the host is the IPv4 endpoint's own label under the
    /// service's dual-stack domain ([`dual_stack_host`]) — the same endpoint, reached
    /// over both families, rather than a second one.
    ///
    /// A VPC-access domain has no public host of either version: it publishes both of
    /// its hosts in [`Self::vpc_access_endpoints`] instead, the dual-stack one under
    /// `vpcv2`.
    pub fn public_endpoint_v2(&self) -> Option<String> {
        if self.vpc_options.is_some()
            || self.ip_address_type.as_deref() != Some(IP_ADDRESS_TYPE_DUALSTACK)
        {
            return None;
        }
        let label = self.endpoint.split('.').next()?;
        (!label.is_empty()).then(|| dual_stack_host(label, &self.region))
    }

    /// This domain as `DomainStatus` — what `CreateDomain`, `DescribeDomain` and
    /// `DeleteDomain` answer with.
    ///
    /// `local_endpoint` is the emulator's own endpoint the READING request reached, when it
    /// reached that rather than one of AWS's service hosts
    /// ([`local_endpoint_host`]): every address this status publishes for the domain's data
    /// plane is then that endpoint with the path this service serves the domain at
    /// ([`announced_domain_endpoint`]), because a caller that resolves nothing but the
    /// emulator's endpoint cannot reach an AWS-shaped host. It is read per CALL rather than
    /// stored, so a domain created by one caller (a resource provisioner reaching this
    /// service in-process, say) publishes to each reader the address that reader can use.
    pub fn to_status(&self, local_endpoint: Option<&str>) -> DomainStatus {
        // Where this domain is reachable decides which endpoint members it reports:
        // the map is present exactly for a VPC-access domain, and the public endpoint
        // exactly for the domain that has one.
        let vpc_access_endpoints = self.vpc_access_endpoints().map(|endpoints| {
            endpoints
                .into_iter()
                .map(|(key, host)| {
                    (
                        key,
                        announced_domain_endpoint(&host, &self.domain_name, local_endpoint),
                    )
                })
                .collect::<BTreeMap<String, String>>()
        });
        let announced =
            |host: &str| announced_domain_endpoint(host, &self.domain_name, local_endpoint);
        DomainStatus {
            domain_id: self.domain_id.clone(),
            domain_name: self.domain_name.clone(),
            arn: self.arn.clone(),
            created: Some(self.created),
            deleted: Some(self.deleted),
            // `Endpoint` is the "domain-specific endpoint used to submit index,
            // search, and data upload requests" — the PUBLIC one, which a domain
            // provisioned inside a VPC does not have: it is reached on the hosts
            // `Endpoints` below carries instead.
            endpoint: vpc_access_endpoints
                .is_none()
                .then(|| announced(&self.endpoint)),
            // `EndpointV2` is the dual-stack PUBLIC endpoint, which a domain whose
            // `IPAddressType` is `dualstack` is provisioned beside its IPv4 one
            // ([`Self::public_endpoint_v2`]); a VPC-access domain has no public host of
            // either version and reports its dual-stack host in `Endpoints` below,
            // under `vpcv2`.
            endpoint_v2: self
                .public_endpoint_v2()
                .map(|host| announced(&host)),
            // "The key-value pair that exists if the OpenSearch Service domain uses VPC
            // endpoints": present for a VPC-access domain and absent for a public one.
            // A service-managed VPC endpoint is a resource of its own, reported by
            // `DescribeVpcEndpoints` rather than here.
            endpoints: vpc_access_endpoints,
            // The dual-stack hosted zone is a Route 53 zone of the service's own,
            // published by a part of AWS this emulator does not model, so its id stays
            // absent rather than being invented.
            domain_endpoint_v2_hosted_zone_id: None,
            // A domain applying a service software deployment is processing too, even
            // when no configuration change is in flight.
            processing: Some(self.processing || self.service_software.deploying()),
            // An engine version upgrade in flight is reported here rather than as a
            // configuration change: `UpgradeProcessing` is "true if OpenSearch Service
            // is in the process of a version upgrade".
            upgrade_processing: Some(self.upgrade_in_flight()),
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
            // "Log publishing options for the domain": the member reports the log
            // types the domain publishes, so a domain that publishes none omits it
            // rather than reporting an empty map — presence is how a caller tells a
            // domain with log publishing from one without. The CONFIGURATION container
            // `to_config` reports is a different thing: every domain has one, empty or
            // not.
            log_publishing_options: (!self.log_publishing_options.is_empty()).then(|| {
                self.log_publishing_options
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
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
            domain_processing_status: Some(self.reported_processing_status().to_string()),
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
            // Every domain has a log-publishing CONFIGURATION, which is empty until a
            // call puts a log type in it: the container is reported with the map the
            // domain holds — empty rather than absent — beside the option's own status,
            // which every container of this descriptor carries.
            log_publishing_options: Some(LogPublishingOptionsStatus {
                options: Some(
                    self.log_publishing_options
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
                status: self.option_status(OPT_LOG_PUBLISHING_OPTIONS),
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
    /// the provisioning the clock owes it ([`DOMAIN_WORK`]) falls due, and no node
    /// exists before then. A later configuration change leaves the running cluster
    /// in place, so a [`PROCESSING_STATUS_MODIFYING`] domain is up.
    pub fn cluster_is_up(&self) -> bool {
        self.created && self.processing_status != PROCESSING_STATUS_CREATING
    }

    /// The instant this domain's cluster is up, or `None` when it already is — what a
    /// caller waiting for the domain to become active waits for.
    ///
    /// A domain that is still being provisioned carries the instant its provisioning
    /// falls due ([`Self::processing_until`]), which is when its cluster comes up.
    pub fn cluster_up_at(&self) -> Option<SystemTime> {
        if self.cluster_is_up() {
            return None;
        }
        self.processing_until
    }

    /// `DomainStatus$DomainProcessingStatus`: where this domain is in its own
    /// lifecycle, one of `DomainProcessingStatusType`'s values.
    ///
    /// Derived in ONE place so the status a descriptor reports and the status the
    /// state gates below read can never disagree: a service software deployment is
    /// work on the domain that no configuration change records, so a deploying domain
    /// is [`PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE`] whatever its last
    /// configuration change left behind.
    pub fn reported_processing_status(&self) -> &str {
        if self.service_software.deploying() {
            PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE
        } else {
            &self.processing_status
        }
    }

    /// Whether this domain has work in flight: it is being provisioned, a
    /// configuration change of it is being applied, an engine version upgrade is
    /// running or a service software deployment is being deployed.
    ///
    /// Read AFTER [`Self::settle`] has materialised whatever the clock owed the
    /// record, so work whose span has passed is no longer in flight.
    pub fn work_in_flight(&self) -> bool {
        self.processing
            || self.service_software.deploying()
            || self.upgrade_in_flight()
            || self.changes.iter().any(ConfigChange::in_progress)
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

/// `NodeType`'s `Data`: a data node, the role `ClusterConfig$InstanceCount` and
/// `InstanceType` configure.
pub const NODE_TYPE_DATA: &str = "Data";

/// `NodeType`'s `Master`: a dedicated master node, the role
/// `ClusterConfig$DedicatedMasterCount` and `DedicatedMasterType` configure.
pub const NODE_TYPE_MASTER: &str = "Master";

/// `NodeType`'s `Ultrawarm`: a warm node, the role `ClusterConfig$WarmCount` and
/// `WarmType` configure. `NodeType` is documented as saying "whether the nodes is a
/// data, master, or UltraWarm node", and `OpenSearchWarmPartitionInstanceType` names
/// only `ultrawarm1` instances, so a warm node is reported under this value.
pub const NODE_TYPE_ULTRAWARM: &str = "Ultrawarm";

/// How many nodes of ONE role this service reports and addresses.
///
/// This is the emulator's OWN bound on the node census — it models neither Service
/// Quotas nor an account's adjustments — set at the number OpenSearch Service
/// publishes as its default data-nodes-per-domain quota. The per-role counts are
/// request fields (`CreateDomain` and `UpdateDomainConfig` persist the caller's
/// `ClusterConfig` verbatim, and `IntegerClass` carries no `@range`), so without this
/// bound one create could size an unbounded response; nothing a caller sends may do
/// that.
pub const NODE_CENSUS_LIMIT: i32 = 80;

/// The `NodeId` of the `index`-th node of one role.
///
/// DERIVED from the domain the node belongs to, the role it fills and its index
/// within that role as a name-based v5 UUID — the way this service derives every id
/// that has to be the same on the next call — so no two nodes share an id and the id
/// a caller read in one call is the id the next call reports, which a freshly
/// generated id would not be. The hyphen-free form's 32 hexadecimal digits sit inside
/// `NodeId`'s `@length(min: 10, max: 40)`.
///
/// The ONE derivation every operation that names a node reads: the census
/// (`DescribeDomainNodes`) publishes these ids and the maintenance surface resolves
/// the `NodeId` a caller sends against them, so a node id taken from the census is
/// the node id maintenance accepts.
pub fn node_id(domain_id: &str, node_type: &str, index: usize) -> String {
    ::uuid::Uuid::new_v5(
        &::uuid::Uuid::NAMESPACE_OID,
        format!("opensearch/domainNode/{domain_id}/{node_type}/{index}").as_bytes(),
    )
    .simple()
    .to_string()
}

/// How many nodes of one role the census holds: the count the record carries,
/// bounded by [`NODE_CENSUS_LIMIT`], and none at all for a count the record carries
/// no value for or a negative one — an unknown node is not a node.
pub fn census_size(count: Option<i32>) -> usize {
    count
        .filter(|count| *count >= 0)
        .map(|count| count.min(NODE_CENSUS_LIMIT))
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0)
}

/// The ids of `domain`'s data nodes, in the order and under the ids the node census
/// publishes them.
pub fn data_node_ids(domain: &Domain) -> Vec<String> {
    (0..census_size(domain.cluster_config.instance_count))
        .map(|index| node_id(&domain.domain_id, NODE_TYPE_DATA, index))
        .collect()
}

// ---------------------------------------------------------------------------
// Node maintenance
//
// `StartDomainMaintenance` "starts the node maintenance process on the data
// node", `GetDomainMaintenanceStatus` reports how that run is going. One record
// below, held on the domain whose node it ran on.
// ---------------------------------------------------------------------------

/// The smallest number of data nodes a maintenance action can take one node out of
/// service in.
///
/// Maintaining a node stops the process running on it, so the nodes left have to
/// carry the cluster on their own: a majority of them has to remain, which
/// `n - 1 > n / 2` first satisfies at three. A role with fewer nodes than this has no
/// node that can be taken out, so no node of it is eligible for maintenance.
pub const MAINTAINABLE_DATA_NODES: i32 = 3;

/// `MaintenanceStatus`'s `PENDING` value: the action has been accepted and has not
/// begun yet.
pub const MAINTENANCE_STATUS_PENDING: &str = "PENDING";

/// `MaintenanceStatus`'s `IN_PROGRESS` value: the action is running on the node.
pub const MAINTENANCE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `MaintenanceStatus`'s `COMPLETED` value: the action has finished.
pub const MAINTENANCE_STATUS_COMPLETED: &str = "COMPLETED";

/// How long each step of a maintenance action takes.
///
/// The same reasoning as [`PROVISIONING`]: AWS takes as long as the node takes, the
/// emulator chooses a span, and it is deliberately NOT zero, so a caller reading
/// straight after the call still observes the action `PENDING` and then
/// `IN_PROGRESS`. The due instants are stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const MAINTENANCE_STEP: Duration = Duration::from_millis(500);

/// How many maintenance actions a domain's history keeps — the emulator's own bound
/// on the store, like [`CHANGE_HISTORY`], so a caller starting actions forever cannot
/// grow it without end.
pub const MAINTENANCE_HISTORY: usize = 50;

/// One node maintenance action on a domain: the action asked for, the node it runs
/// on and how far through its run it is.
#[derive(Debug, Clone)]
pub struct Maintenance {
    /// `StartDomainMaintenanceResponse$MaintenanceId`: "the request ID of requested
    /// action", which `GetDomainMaintenanceStatus` addresses the action by.
    pub maintenance_id: String,
    /// One of `MaintenanceType`'s values, as the request named it.
    pub action: String,
    /// The `NodeId` of the data node this action runs on.
    pub node_id: String,
    /// One of `MaintenanceStatus`'s values; see [`MAINTENANCE_STATUS_PENDING`].
    pub status: String,
    /// `CreatedAt`: the instant of the call that started the action.
    pub created_at: SystemTime,
    /// `UpdatedAt`: the instant this action's status last moved.
    pub updated_at: SystemTime,
    /// When the clock owes this action its start; `None` once it has begun.
    pub starts_at: Option<SystemTime>,
    /// When the clock owes this action its completion; `None` once it has completed.
    pub completes_at: Option<SystemTime>,
}

impl Maintenance {
    /// Materialise the status moves the clock owes this action by `now`.
    ///
    /// Each move is dated at the DUE instant rather than at `now`, so the `UpdatedAt`
    /// a caller reads does not move with the moment it happened to look.
    fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.starts_at.filter(|due| now >= *due) {
            self.starts_at = None;
            self.status = MAINTENANCE_STATUS_IN_PROGRESS.to_string();
            self.updated_at = due;
        }
        if let Some(due) = self.completes_at.filter(|due| now >= *due) {
            self.completes_at = None;
            self.status = MAINTENANCE_STATUS_COMPLETED.to_string();
            self.updated_at = due;
        }
    }

    /// This record as `DomainMaintenanceDetails` — the element shape
    /// `ListDomainMaintenances` reports, member for member the projection
    /// `GetDomainMaintenanceStatus` answers with, so a listing and a read of one
    /// action cannot disagree.
    ///
    /// `StatusMessage` is "the status message of the maintenance action": this
    /// service's own wording for one is not published, and the status itself is
    /// reported in `Status`, so none is invented.
    pub fn to_details(&self, domain_name: &str) -> DomainMaintenanceDetails {
        DomainMaintenanceDetails {
            maintenance_id: Some(self.maintenance_id.clone()),
            domain_name: Some(domain_name.to_string()),
            action: Some(self.action.clone()),
            node_id: Some(self.node_id.clone()),
            status: Some(self.status.clone()),
            status_message: None,
            created_at: Some(datetime(self.created_at)),
            updated_at: Some(datetime(self.updated_at)),
        }
    }
}

/// The `RequestId` a maintenance action is addressed by: a `GUID` like every other id
/// this service hands out ([`new_guid`]), well inside `RequestId`'s
/// `@length(min: 1, max: 100)`, so no two actions share one.
pub fn new_maintenance_id() -> String {
    new_guid()
}

/// Whether `node_id` names a data node of `domain` that a maintenance action can be
/// started on.
///
/// Both halves of the eligibility: the id has to be one the domain's node census
/// publishes for its data nodes, and the role has to be able to lose one of them
/// ([`MAINTAINABLE_DATA_NODES`]).
pub fn maintainable_data_node(domain: &Domain, node_id: &str) -> bool {
    domain
        .cluster_config
        .instance_count
        .is_some_and(|count| count >= MAINTAINABLE_DATA_NODES)
        && data_node_ids(domain).iter().any(|id| id == node_id)
}

// ---------------------------------------------------------------------------
// The Reserved Instance offering catalog
// ---------------------------------------------------------------------------

/// `ReservedInstancePaymentOption`'s `ALL_UPFRONT` value: the whole term is paid as
/// one upfront charge, so the reservation's recurring charge is a zero amount.
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

/// How many offerings ONE page of `DescribeReservedInstanceOfferings` carries when the
/// request names no `MaxResults`.
///
/// `MaxResults` is `@default(0)`: a request that names no page size has asked for no
/// PARTICULAR one, and the service answers with its own default page rather than with
/// the whole catalog — which is why the recorded first answer of this catalog carries a
/// continuation token. The model publishes no default, so this is the emulator's choice
/// of it, and it is the largest page a caller can ask for (`MaxResults` is
/// `@range(max: 100)`): a service does not answer an unbounded request with more than it
/// will answer a bounded one with. The catalog spans every instance type times every
/// term times every payment option, so it is far larger than this and the listing really
/// does hand out tokens.
pub const RESERVED_INSTANCE_OFFERING_PAGE: usize = 100;

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
    let factor = size_normalization_factor(instance_type)?;
    let class = instance_family(instance_type)?.chars().next()?;
    let rate = CLASS_HOURLY_RATE_PER_UNIT
        .iter()
        .find(|(letter, _)| *letter == class)
        .map(|(_, rate)| *rate)?;
    Some(rate * factor)
}

/// The size normalization factor of one instance type, from
/// [`SIZE_NORMALIZATION_FACTORS`]: how many units of its instance class one instance of
/// that size is. It is the ONE scale this crate expresses a per-size quantity against,
/// so a bigger size of a family is bigger by the same factor wherever the size matters.
///
/// `None` for a value that is not `<family>.<size>.search` with a published size token,
/// which no value of `OpenSearchPartitionInstanceType` is.
pub fn size_normalization_factor(instance_type: &str) -> Option<f64> {
    let size = instance_type.split('.').nth(1)?;
    SIZE_NORMALIZATION_FACTORS
        .iter()
        .find(|(token, _)| *token == size)
        .map(|(_, factor)| *factor)
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
/// the reservation has already paid for the hours. So `ALL_UPFRONT` carries the whole
/// term as a fixed price and nothing hourly, `NO_UPFRONT` everything hourly and no
/// fixed price, and `PARTIAL_UPFRONT` half the term each way.
///
/// `RecurringCharges` carries ONE entry whatever the payment option: "the recurring
/// charge to your account" is billed per hour of the term for every offering, so an
/// offering whose term is paid up front publishes that charge with a ZERO amount — the
/// charge exists and is nothing — rather than publishing no charge at all, which would
/// leave a caller summing an empty list to tell a prepaid offering from one whose
/// hourly rate the answer simply did not report.
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
    let recurring_charges = vec![RecurringCharge {
        recurring_charge_amount: Some(hourly(recurring_hourly)),
        recurring_charge_frequency: Some(RECURRING_CHARGE_HOURLY.to_string()),
    }];
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

/// Where one offering sits in the catalog the service PUBLISHES: the instance type it
/// reserves, in the order the published instance-type catalog lists them
/// (`OpenSearchPartitionInstanceType`), then the term shortest first, then the payment
/// option in the order `ReservedInstancePaymentOption` declares them.
///
/// A price list is published per instance type, so that is the key a page of it is read
/// along: a caller walking the catalog sees one instance type's terms and payment options
/// together, and the same order on every call. The id an entry is STORED under is a digest
/// of what the entry is, which orders the catalog by nothing a caller can read.
fn offering_catalog_rank(offering: &ReservedInstanceOffering) -> (usize, i32, usize) {
    let position = |values: &[&str], value: Option<&str>| {
        values
            .iter()
            .position(|candidate| Some(*candidate) == value)
            .unwrap_or(values.len())
    };
    (
        position(
            OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES,
            offering.instance_type.as_deref(),
        ),
        offering.duration,
        position(
            RESERVED_INSTANCE_PAYMENT_OPTION_VALUES,
            offering.payment_option.as_deref(),
        ),
    )
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
// Which instance types an engine version offers, which node roles each of them
// serves and which features it supports is content published BY the service: no
// caller creates an instance type, and the answer depends only on the release
// asked about. [`INSTANCE_TYPE_CATALOG`] below IS that published content, so it is
// the one record of which instance types exist, in the order the service publishes
// them, and two reads of one release report the same types with the same roles and
// the same features.
//
// It is the catalog rather than `OpenSearchPartitionInstanceType` because the two
// are not the same set: the service offers instance families that enum does not
// list, publishes the ones it does list in an order of its own, and retires a
// family it no longer provisions, none of which a modelled value set can carry.
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

/// `InstanceTypeDetails$InstanceRole` for an instance that can serve as a dedicated
/// coordinator node: one that routes and merges a request across the cluster and holds
/// none of its data. A family whose instances can be provisioned in that role reports
/// it beside the roles it serves, which is what tells a caller the role is available on
/// the type at all.
pub const INSTANCE_ROLE_COORDINATOR: &str = "coordinator";

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

/// The OpenSearch release the SECOND OpenSearch-optimized generation is offered from:
/// a later family of the same line, so it is offered from a later release than
/// [`OPENSEARCH_OPTIMIZED_SINCE`] and, like it, on no Elasticsearch release. It is the
/// one family the recorded catalogs of two OpenSearch releases differ by, which is what
/// fixes the gate at a release rather than at the engine.
const OPENSEARCH_OPTIMIZED_GEN2_SINCE: (u32, u32) = (2, 17);

/// The OpenSearch release the seventh-generation Intel families are offered from.
///
/// That generation arrived after the engines forked, so no Elasticsearch release offers
/// it and the gate is an OpenSearch one. AWS publishes no machine-readable boundary for
/// an instance family, so the release itself is the emulator's own statement of
/// published content, like the figures in the price and limit tables above: what these
/// gates fix is that a release predating a family does not offer it, that an
/// Elasticsearch release offers none of the generations that postdate the fork, and
/// that those generations are offered in the order they arrived — this one, then the
/// Graviton4 compute family ([`GRAVITON4_SINCE_OPENSEARCH`]), then the second
/// OpenSearch-optimized generation, which is the one ordering the recorded catalogs of
/// two OpenSearch releases show.
const SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH: (u32, u32) = (2, 13);

/// The OpenSearch release the Graviton4 compute family is offered from — see
/// [`SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH`] for what this gate fixes and what in
/// it is the emulator's own statement.
const GRAVITON4_SINCE_OPENSEARCH: (u32, u32) = (2, 15);

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

/// The node roles a general-purpose, compute-optimized or memory-optimized family
/// serves: a node of one holds the domain's data, is master-eligible, and can be
/// provisioned as a dedicated coordinator node.
const ROLES_WITH_COORDINATOR: &[&str] = &[
    INSTANCE_ROLE_DATA,
    INSTANCE_ROLE_MASTER,
    INSTANCE_ROLE_COORDINATOR,
];

/// The roles a family that is master-eligible but runs no dedicated coordinator node
/// serves — a coordinator node holds none of the domain's data, so a storage-optimized
/// family, a burstable one and a previous generation publish these two.
const ROLES_DATA_AND_MASTER: &[&str] = &[INSTANCE_ROLE_DATA, INSTANCE_ROLE_MASTER];

/// The roles an OpenSearch-optimized family serves: its instances hold the domain's
/// data and are never master-eligible.
const ROLES_DATA_ONLY: &[&str] = &[INSTANCE_ROLE_DATA];

/// The roles the UltraWarm family serves: the warm tier, and nothing else.
const ROLES_ULTRAWARM_ONLY: &[&str] = &[INSTANCE_ROLE_ULTRAWARM];

/// Which engine releases offer one instance family.
#[derive(Debug, Clone, Copy)]
enum OfferedReleases {
    /// Every release the service publishes — a family with no published engine
    /// prerequisite, which is most of the catalog: the recorded catalog of a current
    /// OpenSearch release carries previous-generation entries too, so the catalog is
    /// not filtered by instance generation.
    Every,
    /// From an Elasticsearch release onwards — and so on every OpenSearch release,
    /// which continues the Elasticsearch 7 line.
    SinceElasticsearch((u32, u32)),
    /// From an OpenSearch release onwards, and on no Elasticsearch release: a family
    /// the service publishes for the OpenSearch engine alone.
    SinceOpenSearch((u32, u32)),
}

/// One family of the published instance-type catalog: the instance types the service
/// offers it as, the node roles an instance of it serves, and the releases that offer
/// it.
#[derive(Debug, Clone, Copy)]
struct CatalogFamily {
    /// The family token — the `<family>` of `<family>.<size>.search`.
    family: &'static str,
    /// The instance types the family is published as, smallest size first. Each one's
    /// size is a token of [`SIZE_NORMALIZATION_FACTORS`], so every published type has a
    /// size the rest of this crate can scale a per-size quantity against.
    instance_types: &'static [&'static str],
    /// The roles an instance of the family serves, in the order they are reported.
    roles: &'static [&'static str],
    /// The releases that offer the family.
    releases: OfferedReleases,
}

impl CatalogFamily {
    /// Whether `version` offers this family.
    fn offered_on(&self, version: EngineVersion) -> bool {
        match self.releases {
            OfferedReleases::Every => true,
            OfferedReleases::SinceElasticsearch(release) => version.since_elasticsearch(release),
            OfferedReleases::SinceOpenSearch(release) => version.opensearch_at_least(release),
        }
    }
}

/// The instance families AWS publishes as PREVIOUS generation for this service — the
/// generation before the current one, which it still offers. The published catalog
/// lists them after the current generations, which is what [`catalog_rank`] uses them
/// for. [`PREVIOUS_GENERATION_EBS_FAMILIES`] is the subset of them the magnetic volume
/// sub-type is published for.
const PREVIOUS_GENERATION_FAMILIES: [&str; 8] = ["c4", "d2", "i2", "m3", "m4", "r3", "r4", "t2"];

/// Where one family sits in the published catalog: the current generations first, then
/// the UltraWarm tier — which holds no hot-tier node and is published apart from them —
/// then the previous generations, and inside a group by family name.
///
/// The GROUPING is the one AWS's supported-instance-types documentation for this
/// service publishes: the current generations, the warm tier and the previous
/// generations, in that order. The order of the families WITHIN a group is by name,
/// which is the emulator's own statement — that page groups a generation's families by
/// instance class rather than naming an order among them, and the service's own
/// ordering is not recoverable from anything it publishes. Computing the rank here
/// rather than writing a sequence out is what makes the published order a property of
/// the CATALOG: adding a family puts it where the grouping puts it, not where it was
/// appended.
fn catalog_rank(entry: &CatalogFamily) -> (u8, &'static str) {
    let group = if entry.family == ULTRAWARM_FAMILY {
        1
    } else if PREVIOUS_GENERATION_FAMILIES.contains(&entry.family) {
        2
    } else {
        0
    };
    (group, entry.family)
}

/// The instance-type catalog the service publishes, one entry per instance FAMILY.
///
/// It is published in [`catalog_rank`]'s order — the generation grouping AWS's
/// supported-instance-types documentation for this service publishes, by family name
/// within a group — and each family's instance types smallest size first, so the order
/// of the rows below is not itself the order of a listing.
///
/// This is the service's own published content, so it is written out here rather than
/// derived from `OpenSearchPartitionInstanceType`: the service offers families that
/// enum does not list at all, and a modelled value set carries neither the engine
/// releases that offer a family nor the node roles an instance of it serves. It
/// carries every value of the modelled enum and the current generations the service's
/// answers add to it.
///
/// What is NOT published anywhere machine-readable, and is therefore the emulator's own
/// statement of published content exactly as [`CLASS_HOURLY_RATE_PER_UNIT`] and
/// [`MAXIMUM_DATA_NODE_COUNT`] are: the sizes each family is offered in, and the engine
/// release a family carries no documented prerequisite for (such a family is offered on
/// every release the service publishes). The service's own internal ordering of its
/// catalog is not recoverable from anything it publishes either, so the documented
/// order above is what the catalog is published in, and it is stable: element N of a
/// release's listing is the same instance type on every call.
///
/// Every other reader of an instance type — its roles, its limits, the validation of a
/// cluster configuration — reads this one table, so no two of them can disagree about
/// which instance types exist. Every value of the modelled enum has to be one of them,
/// which [`publishes_every_modelled_instance_type`] holds.
const INSTANCE_TYPE_CATALOG: &[CatalogFamily] = &[
    // General purpose.
    CatalogFamily {
        family: "m7i",
        instance_types: &[
            "m7i.large.search",
            "m7i.xlarge.search",
            "m7i.2xlarge.search",
            "m7i.4xlarge.search",
            "m7i.8xlarge.search",
            "m7i.12xlarge.search",
            "m7i.16xlarge.search",
            "m7i.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "m6g",
        instance_types: &[
            "m6g.large.search",
            "m6g.xlarge.search",
            "m6g.2xlarge.search",
            "m6g.4xlarge.search",
            "m6g.8xlarge.search",
            "m6g.12xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "m5",
        instance_types: &[
            "m5.large.search",
            "m5.xlarge.search",
            "m5.2xlarge.search",
            "m5.4xlarge.search",
            "m5.12xlarge.search",
            "m5.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "m4",
        instance_types: &[
            "m4.large.search",
            "m4.xlarge.search",
            "m4.2xlarge.search",
            "m4.4xlarge.search",
            "m4.10xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "m3",
        instance_types: &[
            "m3.medium.search",
            "m3.large.search",
            "m3.xlarge.search",
            "m3.2xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // Compute optimized.
    CatalogFamily {
        family: "c8g",
        instance_types: &[
            "c8g.large.search",
            "c8g.xlarge.search",
            "c8g.2xlarge.search",
            "c8g.4xlarge.search",
            "c8g.8xlarge.search",
            "c8g.12xlarge.search",
            "c8g.16xlarge.search",
            "c8g.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "c6g",
        instance_types: &[
            "c6g.large.search",
            "c6g.xlarge.search",
            "c6g.2xlarge.search",
            "c6g.4xlarge.search",
            "c6g.8xlarge.search",
            "c6g.12xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "c5",
        instance_types: &[
            "c5.large.search",
            "c5.xlarge.search",
            "c5.2xlarge.search",
            "c5.4xlarge.search",
            "c5.9xlarge.search",
            "c5.18xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "c4",
        instance_types: &[
            "c4.large.search",
            "c4.xlarge.search",
            "c4.2xlarge.search",
            "c4.4xlarge.search",
            "c4.8xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // Memory optimized.
    CatalogFamily {
        family: "r7i",
        instance_types: &[
            "r7i.large.search",
            "r7i.xlarge.search",
            "r7i.2xlarge.search",
            "r7i.4xlarge.search",
            "r7i.8xlarge.search",
            "r7i.12xlarge.search",
            "r7i.16xlarge.search",
            "r7i.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "r6gd",
        instance_types: &[
            "r6gd.large.search",
            "r6gd.xlarge.search",
            "r6gd.2xlarge.search",
            "r6gd.4xlarge.search",
            "r6gd.8xlarge.search",
            "r6gd.12xlarge.search",
            "r6gd.16xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "r6g",
        instance_types: &[
            "r6g.large.search",
            "r6g.xlarge.search",
            "r6g.2xlarge.search",
            "r6g.4xlarge.search",
            "r6g.8xlarge.search",
            "r6g.12xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "r5",
        instance_types: &[
            "r5.large.search",
            "r5.xlarge.search",
            "r5.2xlarge.search",
            "r5.4xlarge.search",
            "r5.12xlarge.search",
            "r5.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "r4",
        instance_types: &[
            "r4.large.search",
            "r4.xlarge.search",
            "r4.2xlarge.search",
            "r4.4xlarge.search",
            "r4.8xlarge.search",
            "r4.16xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "r3",
        instance_types: &[
            "r3.large.search",
            "r3.xlarge.search",
            "r3.2xlarge.search",
            "r3.4xlarge.search",
            "r3.8xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // Storage optimized: an instance of one holds the domain's data on storage built
    // into it, and a coordinator node holds none, so it is not provisioned on one.
    CatalogFamily {
        family: "i3",
        instance_types: &[
            "i3.large.search",
            "i3.xlarge.search",
            "i3.2xlarge.search",
            "i3.4xlarge.search",
            "i3.8xlarge.search",
            "i3.16xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "i2",
        instance_types: &["i2.xlarge.search", "i2.2xlarge.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "d2",
        instance_types: &[
            "d2.xlarge.search",
            "d2.2xlarge.search",
            "d2.4xlarge.search",
            "d2.8xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // OpenSearch optimized.
    CatalogFamily {
        family: "oi2",
        instance_types: &[
            "oi2.medium.search",
            "oi2.large.search",
            "oi2.xlarge.search",
            "oi2.2xlarge.search",
            "oi2.4xlarge.search",
            "oi2.8xlarge.search",
            "oi2.12xlarge.search",
            "oi2.16xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(OPENSEARCH_OPTIMIZED_GEN2_SINCE),
    },
    CatalogFamily {
        family: "or1",
        instance_types: &[
            "or1.medium.search",
            "or1.large.search",
            "or1.xlarge.search",
            "or1.2xlarge.search",
            "or1.4xlarge.search",
            "or1.8xlarge.search",
            "or1.12xlarge.search",
            "or1.16xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(OPENSEARCH_OPTIMIZED_SINCE),
    },
    // Burstable.
    CatalogFamily {
        family: "t4g",
        instance_types: &["t4g.small.search", "t4g.medium.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "t3",
        instance_types: &[
            "t3.nano.search",
            "t3.micro.search",
            "t3.small.search",
            "t3.medium.search",
            "t3.large.search",
            "t3.xlarge.search",
            "t3.2xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "t2",
        instance_types: &["t2.micro.search", "t2.small.search", "t2.medium.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // The UltraWarm tier.
    CatalogFamily {
        family: ULTRAWARM_FAMILY,
        instance_types: &[
            "ultrawarm1.medium.search",
            "ultrawarm1.large.search",
            "ultrawarm1.xlarge.search",
        ],
        roles: ROLES_ULTRAWARM_ONLY,
        releases: OfferedReleases::SinceElasticsearch(ULTRAWARM_SINCE_ELASTICSEARCH),
    },
];

/// The family token of an instance type, which is `<family>.<size>.search`; `None` for
/// a value carrying no family at all.
pub fn instance_family(instance_type: &str) -> Option<&str> {
    instance_type
        .split('.')
        .next()
        .filter(|family| !family.is_empty())
}

/// The catalog entry `instance_type` belongs to, and `None` for a value the service
/// publishes no instance type for — a family the catalog does not carry, a size that
/// family is not offered in, or a value that is not `<family>.<size>.search` at all.
fn catalog_family(instance_type: &str) -> Option<&'static CatalogFamily> {
    let family = instance_family(instance_type)?;
    INSTANCE_TYPE_CATALOG
        .iter()
        .find(|entry| entry.family == family && entry.instance_types.contains(&instance_type))
}

/// Whether the service publishes `instance_type` at all, whatever the release: the
/// catalog is the record of which instance types exist, so a value it does not carry
/// names no instance type.
pub fn publishes_instance_type(instance_type: &str) -> bool {
    catalog_family(instance_type).is_some()
}

/// Whether `version` offers `instance_type`.
///
/// An instance type is offered when the catalog publishes it and the release asked
/// about is one its family is offered on, so a family the service introduced for a
/// later release is not offered on an earlier one.
pub fn offers_instance_type(version: EngineVersion, instance_type: &str) -> bool {
    catalog_family(instance_type).is_some_and(|entry| entry.offered_on(version))
}

/// Every instance type `version` offers, in the order the catalog is published:
/// family by family in [`catalog_rank`]'s order, each family's instance types smallest
/// size first, less the families this release does not offer.
///
/// The order is the catalog's and is stable, so a paged read resumes where the previous
/// page stopped and element N of the listing is the same instance type on every call.
pub fn offered_instance_types(version: EngineVersion) -> Vec<&'static str> {
    let mut families: Vec<&'static CatalogFamily> = INSTANCE_TYPE_CATALOG
        .iter()
        .filter(|entry| entry.offered_on(version))
        .collect();
    families.sort_by_key(|entry| catalog_rank(entry));
    families
        .into_iter()
        .flat_map(|entry| entry.instance_types.iter().copied())
        .collect()
}

/// Whether the catalog publishes every value of `OpenSearchPartitionInstanceType`.
///
/// The modelled value set is the model's own record of which instance types exist, so
/// the service's catalog carries all of it and the families the service added since: a
/// value the catalog did not carry would name no instance type at all, and the
/// operations that resolve one against the catalog would refuse it. Held by this
/// service's `instance_type_catalog` suite.
pub fn publishes_every_modelled_instance_type() -> bool {
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES
        .iter()
        .all(|instance_type| publishes_instance_type(instance_type))
}

/// The roles an instance of `instance_type` can serve, in the order the catalog
/// publishes them, and NO role for a type the catalog does not publish — which names
/// no instance type, so there is no instance to serve a role.
///
/// The roles are the catalog entry's: the UltraWarm family serves the warm tier, an
/// OpenSearch-optimized instance is a data node only, a general-purpose, compute- or
/// memory-optimized instance can also be a dedicated coordinator node, and every other
/// family is a data node and master-eligible. This is the ONE derivation of them, so
/// the roles the instance-type catalog reports for a type are the roles the limit
/// catalog below publishes limits for.
pub fn instance_roles(instance_type: &str) -> Vec<&'static str> {
    catalog_family(instance_type).map_or_else(Vec::new, |entry| entry.roles.to_vec())
}

/// What `instance_type` supports on `version`, as `InstanceTypeDetails`.
///
/// Each feature member is a conjunction of two things: the engine release the feature
/// is available from, and whether the instance family can serve it at all. The roles
/// are the ones the catalog publishes for the type ([`instance_roles`]), so the entry
/// reports the roles an instance of it can actually be provisioned in.
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
    let instance_role = instance_roles(instance_type)
        .into_iter()
        .map(str::to_string)
        .collect();
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

// ---------------------------------------------------------------------------
// The instance-type LIMIT catalog
//
// What an instance type can be scaled to — how many of it a domain holds in each
// role, and what storage attaches to it — is published BY the service alongside
// the instance-type catalog above, so it is DERIVED from the instance type and
// the role rather than stored per account: two reads of one instance type report
// the same limits, and the roles it is reported for are the roles
// [`instance_roles`] gives that type.
// ---------------------------------------------------------------------------

/// `StorageTypeName`'s `ebs`: Elastic Block Store storage attached to the instance. It
/// is the storage type that carries a sub-type and volume limits.
pub const STORAGE_TYPE_EBS: &str = "ebs";

/// `StorageTypeName`'s `instance`: "built-in storage available for the instance", which
/// has no sub-type and no volume limits, because there is no volume to size.
pub const STORAGE_TYPE_INSTANCE: &str = "instance";

/// The key `LimitsByRole` publishes the UltraWarm tier's limits under. That map's own
/// documentation names its three roles `data`, `master` and `ultra_warm`, so the warm
/// key here is NOT [`INSTANCE_ROLE_ULTRAWARM`], which is the value
/// `InstanceTypeDetails$InstanceRole` reports.
pub const LIMITS_ROLE_ULTRAWARM: &str = "ultra_warm";

/// `VolumeType`'s `standard`: a magnetic volume, the one sub-type with its own size cap
/// ([`MAXIMUM_STANDARD_VOLUME_SIZE_GIB`]).
pub const VOLUME_TYPE_STANDARD: &str = "standard";

/// `VolumeType`'s `gp2`: a general-purpose SSD volume whose performance is not
/// provisioned per volume.
pub const VOLUME_TYPE_GP2: &str = "gp2";

/// `VolumeType`'s `gp3`: the general-purpose SSD volume whose IOPS and throughput are
/// provisioned (`EBSOptions$Iops`, `EBSOptions$Throughput`).
pub const VOLUME_TYPE_GP3: &str = "gp3";

/// `VolumeType`'s `io1`: a provisioned-IOPS SSD volume.
pub const VOLUME_TYPE_IO1: &str = "io1";

/// `StorageTypeLimit$LimitName`'s `MinimumVolumeSize`: "minimum volume size that is
/// available for the given storage type".
pub const LIMIT_MINIMUM_VOLUME_SIZE: &str = "MinimumVolumeSize";

/// `StorageTypeLimit$LimitName`'s `MaximumVolumeSize`: "maximum volume size that is
/// available for the given storage type".
pub const LIMIT_MAXIMUM_VOLUME_SIZE: &str = "MaximumVolumeSize";

/// `StorageTypeLimit$LimitName`'s `MinimumIops`.
pub const LIMIT_MINIMUM_IOPS: &str = "MinimumIops";

/// `StorageTypeLimit$LimitName`'s `MaximumIops`.
pub const LIMIT_MAXIMUM_IOPS: &str = "MaximumIops";

/// `StorageTypeLimit$LimitName`'s `MinimumThroughput`.
pub const LIMIT_MINIMUM_THROUGHPUT: &str = "MinimumThroughput";

/// `StorageTypeLimit$LimitName`'s `MaximumThroughput`.
pub const LIMIT_MAXIMUM_THROUGHPUT: &str = "MaximumThroughput";

/// `AdditionalLimit$LimitName`'s `MaximumNumberOfDataNodesSupported`, which "only
/// applies to master nodes and specifies the maximum number of data nodes of a given
/// instance type a master node can support".
pub const LIMIT_DATA_NODES_SUPPORTED: &str = "MaximumNumberOfDataNodesSupported";

/// `AdditionalLimit$LimitName`'s `MaximumNumberOfDataNodesWithoutMasterNode`, which
/// "only applies to data nodes and specifies the maximum number of data nodes of a
/// given instance type can exist without a master node governing them".
pub const LIMIT_DATA_NODES_WITHOUT_MASTER: &str = "MaximumNumberOfDataNodesWithoutMasterNode";

/// The instance families whose data sits on storage built into the instance rather than
/// on an attached volume, so the only storage type they publish is
/// [`STORAGE_TYPE_INSTANCE`].
const INSTANCE_STORAGE_FAMILIES: [&str; 5] = ["d2", "i2", "i3", "r3", "r6gd"];

/// The smallest EBS volume, in GiB, every sub-type publishes — the floor a volume size
/// is below rather than simply smaller than, whatever the instance type.
pub const MINIMUM_VOLUME_SIZE_GIB: i64 = 10;

/// The largest magnetic ([`VOLUME_TYPE_STANDARD`]) volume, in GiB: the one sub-type
/// whose maximum is a flat figure rather than one that scales with the instance.
pub const MAXIMUM_STANDARD_VOLUME_SIZE_GIB: i64 = 100;

/// How many GiB of EBS volume ONE instance-size normalization unit carries, which is
/// what makes the maximum volume size scale with the instance: a `large` (four units of
/// [`SIZE_NORMALIZATION_FACTORS`]) publishes 512 GiB, an `xlarge` 1 TiB, and the
/// largest published size, `24xlarge`, 24 TiB.
///
/// Like [`CLASS_HOURLY_RATE_PER_UNIT`] this is the emulator's OWN figure: AWS publishes
/// a table per instance type, which is not part of the API and not in the Smithy model.
/// What it guarantees is the RELATION — the maximum is monotonic in the instance's size
/// and never below [`SMALLEST_MAXIMUM_VOLUME_SIZE_GIB`].
const VOLUME_SIZE_GIB_PER_UNIT: f64 = 128.0;

/// The smallest maximum any instance type publishes, in GiB: a volume of this size
/// attaches to every published instance type, whatever its size, and only above it does
/// the scale differentiate one instance from another. It is what keeps the published
/// maximum of a SMALL instance from being the narrower of the two bounds a caller is
/// held to — the service's own maximum for a small type is not below this — so the
/// catalog never reports a volume as unavailable that the service would attach.
const SMALLEST_MAXIMUM_VOLUME_SIZE_GIB: i64 = 512;

/// The IOPS range a [`VOLUME_TYPE_GP3`] volume publishes: its 3,000 IOPS baseline is
/// the floor, and 16,000 the most one volume can be provisioned with.
const GP3_IOPS_RANGE: (i64, i64) = (3_000, 16_000);

/// The same for a [`VOLUME_TYPE_IO1`] volume, which is provisioned from 1,000 IOPS.
const IO1_IOPS_RANGE: (i64, i64) = (1_000, 16_000);

/// The throughput range, in MiB/s, a [`VOLUME_TYPE_GP3`] volume publishes: a 125 MiB/s
/// baseline up to 1,000 MiB/s.
const GP3_THROUGHPUT_RANGE_MIB: (i64, i64) = (125, 1_000);

/// The fewest data nodes a domain runs: a single node is a whole domain.
pub const MINIMUM_DATA_NODE_COUNT: i32 = 1;

/// The most data nodes the limit catalog publishes for the data role — the
/// `MaximumInstanceCount` `DescribeInstanceTypeLimits` reports under it.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN published figure rather than one the
/// model carries, and it is ONE figure for every instance type: the service publishes a
/// per-instance-type table, which is not part of the API and not in the model, so there
/// is nothing to read a per-type ceiling from. What this fixes is that the data-node
/// count is bounded and that the bound is above the master one and below the warm one.
pub const MAXIMUM_DATA_NODE_COUNT: i32 = 20;

/// The dedicated-master count a domain runs: a master quorum needs more than one node,
/// and an odd count above five buys nothing, so five is the top.
pub const MASTER_NODE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 2..=5;

/// The UltraWarm node count a domain runs: the warm tier is attached in a pair or more,
/// and like [`MAXIMUM_DATA_NODE_COUNT`] its top is the emulator's own bound.
pub const WARM_NODE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 2..=150;

/// How many data nodes a domain runs before a dedicated master node is needed to govern
/// them — `MaximumNumberOfDataNodesWithoutMasterNode`.
pub const DATA_NODES_WITHOUT_MASTER: i32 = 10;

/// How many data nodes a dedicated master supports, by the size of the master instance:
/// each row is the largest [`SIZE_NORMALIZATION_FACTORS`] factor it covers, with the
/// count. A bigger master governs more data nodes, which is the relation this table
/// fixes; the counts themselves are the emulator's own published figures.
const DATA_NODES_SUPPORTED_BY_MASTER: [(f64, i32); 4] = [
    // Up to and including `large`.
    (4.0, 10),
    // `xlarge`.
    (8.0, 30),
    // `2xlarge`.
    (16.0, 75),
    // Every larger size.
    (f64::INFINITY, 125),
];

/// What ONE instance's EBS attachment carries: the largest volume that attaches to it,
/// the most IOPS it drives and the most throughput it sustains.
///
/// These are bounds of the INSTANCE, not of the volume, and a limit the catalog publishes
/// for a pairing is the LESSER of the two — a volume sub-type's own ceiling is reported
/// only where the instance it attaches to can reach it.
#[derive(Debug, Clone, Copy)]
struct EbsAttachment {
    /// The largest volume, in GiB, one instance of the class takes.
    maximum_volume_size_gib: i64,
    /// The most IOPS one instance of the class drives.
    maximum_iops: i64,
    /// The most throughput, in MiB/s, one instance of the class sustains.
    maximum_throughput_mib: i64,
}

/// The attachment a BURSTABLE instance publishes.
///
/// A burstable instance has no dedicated EBS bandwidth: its attachment bursts from a
/// credit balance, and the figures AWS publishes for that class — a burst of 11,800 IOPS
/// and 260 MiB/s, with a 100 GiB volume ceiling — are every one of them below what the
/// general-purpose volume sub-types publish on their own. That is what makes the
/// burstable instance, rather than the volume, the binding bound, and why it is the one
/// row of this table: every other class reaches the sub-type's own ceilings.
const BURSTABLE_EBS_ATTACHMENT: EbsAttachment = EbsAttachment {
    maximum_volume_size_gib: 100,
    maximum_iops: 11_800,
    maximum_throughput_mib: 260,
};

/// The EBS attachment `instance_type` publishes, and `None` for a class whose attachment
/// reaches every ceiling the volume sub-types publish, so nothing of the instance's own
/// narrows them.
fn ebs_attachment(instance_type: &str) -> Option<EbsAttachment> {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    BURSTABLE_FAMILIES
        .contains(&family)
        .then_some(BURSTABLE_EBS_ATTACHMENT)
}

/// The IOPS range `instance_type` publishes for `sub_type`, and `None` for a sub-type
/// whose performance is not provisioned per volume: `MinimumIops`/`MaximumIops` "can be
/// empty if not applicable", and `EBSOptions$Iops` is "applicable only for the gp3 and
/// provisioned IOPS EBS volume types".
///
/// The ceiling is the lesser of the sub-type's own and what the instance drives; the floor
/// is the sub-type's, and every published attachment drives more than it, so the range
/// never inverts.
fn iops_range(instance_type: &str, sub_type: &str) -> Option<(i64, i64)> {
    let (minimum, maximum) = match sub_type {
        VOLUME_TYPE_GP3 => GP3_IOPS_RANGE,
        VOLUME_TYPE_IO1 => IO1_IOPS_RANGE,
        _ => return None,
    };
    let maximum = ebs_attachment(instance_type)
        .map_or(maximum, |attachment| maximum.min(attachment.maximum_iops));
    Some((minimum, maximum.max(minimum)))
}

/// The throughput range `instance_type` publishes for `sub_type`, bounded by the
/// instance's attachment the same way. `EBSOptions$Throughput` is "applicable only for
/// the gp3 volume type", so every other sub-type publishes none.
fn throughput_range(instance_type: &str, sub_type: &str) -> Option<(i64, i64)> {
    let (minimum, maximum) = match sub_type {
        VOLUME_TYPE_GP3 => GP3_THROUGHPUT_RANGE_MIB,
        _ => return None,
    };
    let maximum = ebs_attachment(instance_type).map_or(maximum, |attachment| {
        maximum.min(attachment.maximum_throughput_mib)
    });
    Some((minimum, maximum.max(minimum)))
}

/// The largest EBS volume, in GiB, `instance_type` publishes for `sub_type`: the
/// instance's size scaled by [`VOLUME_SIZE_GIB_PER_UNIT`], never below
/// [`SMALLEST_MAXIMUM_VOLUME_SIZE_GIB`], capped by what the instance's own attachment
/// takes ([`ebs_attachment`]), and capped for a magnetic volume at
/// [`MAXIMUM_STANDARD_VOLUME_SIZE_GIB`].
pub fn maximum_volume_size(instance_type: &str, sub_type: &str) -> i64 {
    let scaled = size_normalization_factor(instance_type)
        .map_or(SMALLEST_MAXIMUM_VOLUME_SIZE_GIB, |factor| {
            (factor * VOLUME_SIZE_GIB_PER_UNIT).round() as i64
        })
        .max(SMALLEST_MAXIMUM_VOLUME_SIZE_GIB);
    let scaled = ebs_attachment(instance_type).map_or(scaled, |attachment| {
        scaled.min(attachment.maximum_volume_size_gib)
    });
    if sub_type == VOLUME_TYPE_STANDARD {
        scaled.min(MAXIMUM_STANDARD_VOLUME_SIZE_GIB)
    } else {
        scaled
    }
}

/// One `StorageTypeLimit`. Every limit value is a string on the wire
/// (`StorageTypeLimit$LimitValues`), so a published number is rendered here, once.
fn storage_type_limit(name: &str, value: i64) -> StorageTypeLimit {
    StorageTypeLimit {
        limit_name: Some(name.to_string()),
        limit_values: Some(vec![value.to_string()]),
    }
}

/// One `AdditionalLimit`, rendered the same way.
fn additional_limit(name: &str, value: i32) -> AdditionalLimit {
    AdditionalLimit {
        limit_name: Some(name.to_string()),
        limit_values: Some(vec![value.to_string()]),
    }
}

/// The `ebs` storage type `instance_type` publishes for one volume sub-type: the size
/// range every sub-type has, plus the IOPS and throughput ranges of the sub-types those
/// are provisioned for.
///
/// Each bound PAIR is published ceiling before floor — `MaximumVolumeSize` before
/// `MinimumVolumeSize`, `MaximumIops` before `MinimumIops`, `MaximumThroughput` before
/// `MinimumThroughput` — the order the service publishes a storage limit in, and the
/// order `StorageTypeLimit$LimitName`'s own documentation lists the IOPS and the
/// throughput pair in. A caller reading the block positionally therefore reads the
/// ceiling first, which is what makes the pair's order part of the answer rather than a
/// detail of how it is assembled.
fn ebs_storage_type(instance_type: &str, sub_type: &str) -> StorageType {
    let mut limits = vec![
        storage_type_limit(
            LIMIT_MAXIMUM_VOLUME_SIZE,
            maximum_volume_size(instance_type, sub_type),
        ),
        storage_type_limit(LIMIT_MINIMUM_VOLUME_SIZE, MINIMUM_VOLUME_SIZE_GIB),
    ];
    if let Some((minimum, maximum)) = iops_range(instance_type, sub_type) {
        limits.push(storage_type_limit(LIMIT_MAXIMUM_IOPS, maximum));
        limits.push(storage_type_limit(LIMIT_MINIMUM_IOPS, minimum));
    }
    if let Some((minimum, maximum)) = throughput_range(instance_type, sub_type) {
        limits.push(storage_type_limit(LIMIT_MAXIMUM_THROUGHPUT, maximum));
        limits.push(storage_type_limit(LIMIT_MINIMUM_THROUGHPUT, minimum));
    }
    StorageType {
        storage_type_name: Some(STORAGE_TYPE_EBS.to_string()),
        storage_sub_type_name: Some(sub_type.to_string()),
        storage_type_limits: Some(limits),
    }
}

/// The volume sub-types EVERY instance class with an EBS attachment publishes: the
/// general-purpose SSD types, `VolumeType`'s `gp2` and the `gp3` whose IOPS and
/// throughput are provisioned on it.
const GENERAL_PURPOSE_VOLUME_TYPES: [&str; 2] = [VOLUME_TYPE_GP2, VOLUME_TYPE_GP3];

/// The instance families the MAGNETIC sub-type is published for: magnetic storage is
/// offered for the previous-generation instance types alone, so every later family
/// publishes none.
///
/// Which families are previous generation is published BY the service and carried by no
/// part of the model, so this is the emulator's statement of it. The families whose data
/// sits on built-in storage are not among it: they publish no volume at all
/// ([`INSTANCE_STORAGE_FAMILIES`]).
const PREVIOUS_GENERATION_EBS_FAMILIES: [&str; 4] = ["c4", "m3", "m4", "r4"];

/// The `VolumeType` sub-types `instance_type` publishes an `ebs` storage type for, in the
/// order the generated enum lists them.
///
/// The general-purpose SSD types attach to every class. A PROVISIONED-IOPS volume
/// provisions performance the instance itself has to carry, so it is published for every
/// class whose attachment provisions its own — which a burstable instance's, bursting
/// from a credit balance ([`BURSTABLE_EBS_ATTACHMENT`]), does not. The magnetic type is
/// published for the previous-generation families alone.
fn volume_sub_types(instance_type: &str) -> Vec<&'static str> {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    let burstable = BURSTABLE_FAMILIES.contains(&family);
    let previous_generation = PREVIOUS_GENERATION_EBS_FAMILIES.contains(&family);
    VOLUME_TYPE_VALUES
        .iter()
        .copied()
        .filter(|sub_type| match *sub_type {
            VOLUME_TYPE_STANDARD => previous_generation,
            VOLUME_TYPE_IO1 => !burstable,
            _ => GENERAL_PURPOSE_VOLUME_TYPES.contains(sub_type),
        })
        .collect()
}

/// The storage types `instance_type` publishes for the role that holds the domain's
/// DATA.
///
/// A family whose data sits on storage built into the instance publishes that one type
/// and no volume limits; every other family publishes one `ebs` entry per volume sub-type
/// THAT instance type attaches ([`volume_sub_types`]), in the order the enum lists them.
/// The warm tier's storage is not attached to the node at all, so an UltraWarm instance
/// type publishes no storage type rather than an empty list of them.
fn storage_types(instance_type: &str) -> Option<Vec<StorageType>> {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    if family == ULTRAWARM_FAMILY {
        return None;
    }
    if INSTANCE_STORAGE_FAMILIES.contains(&family) {
        return Some(vec![StorageType {
            storage_type_name: Some(STORAGE_TYPE_INSTANCE.to_string()),
            storage_sub_type_name: None,
            storage_type_limits: None,
        }]);
    }
    Some(
        volume_sub_types(instance_type)
            .into_iter()
            .map(|sub_type| ebs_storage_type(instance_type, sub_type))
            .collect(),
    )
}

/// How many data nodes a dedicated master of `instance_type` supports, from
/// [`DATA_NODES_SUPPORTED_BY_MASTER`] — whose last row covers every size, so the
/// fallback is reached only by a value carrying no size token at all.
fn data_nodes_supported_by_master(instance_type: &str) -> i32 {
    let factor = size_normalization_factor(instance_type).unwrap_or_default();
    DATA_NODES_SUPPORTED_BY_MASTER
        .iter()
        .find(|(largest, _)| factor <= *largest)
        .map_or(DATA_NODES_WITHOUT_MASTER, |(_, nodes)| *nodes)
}

/// The limits `instance_type` publishes for ONE of its roles: how many instances of it
/// a domain holds in that role, the storage it can attach, and the additional limit
/// that applies to that role.
///
/// Storage is published for the role that HOLDS the domain's data and for no other: an
/// EBS volume "specifies the type of EBS volumes attached to data nodes"
/// (`EBSOptions$VolumeType`), so a dedicated master — which holds no data and whose
/// configuration carries no volume — publishes no storage type rather than a copy of
/// the data role's, exactly as the warm tier publishes none.
fn role_limits(instance_type: &str, role: &str) -> Limits {
    let (minimum, maximum, additional) = match role {
        INSTANCE_ROLE_MASTER => (
            *MASTER_NODE_COUNT_RANGE.start(),
            *MASTER_NODE_COUNT_RANGE.end(),
            vec![additional_limit(
                LIMIT_DATA_NODES_SUPPORTED,
                data_nodes_supported_by_master(instance_type),
            )],
        ),
        INSTANCE_ROLE_ULTRAWARM => (
            *WARM_NODE_COUNT_RANGE.start(),
            *WARM_NODE_COUNT_RANGE.end(),
            // Neither published additional limit applies to a warm node: one is a
            // master-node limit and the other a data-node one.
            Vec::new(),
        ),
        _ => (
            MINIMUM_DATA_NODE_COUNT,
            MAXIMUM_DATA_NODE_COUNT,
            vec![additional_limit(
                LIMIT_DATA_NODES_WITHOUT_MASTER,
                DATA_NODES_WITHOUT_MASTER,
            )],
        ),
    };
    Limits {
        // Only the data role attaches storage; every other role publishes none.
        storage_types: match role {
            INSTANCE_ROLE_DATA => storage_types(instance_type),
            _ => None,
        },
        instance_limits: Some(InstanceLimits {
            instance_count_limits: Some(InstanceCountLimits {
                minimum_instance_count: minimum,
                maximum_instance_count: maximum,
            }),
        }),
        // A role no published additional limit applies to carries no `AdditionalLimits`
        // member rather than an empty list.
        additional_limits: (!additional.is_empty()).then_some(additional),
    }
}

/// Every limit `instance_type` publishes, keyed by the role it applies to: the
/// `LimitsByRole` map of `DescribeInstanceTypeLimits`.
///
/// That map's own documentation names the three roles it is keyed by — `data`,
/// `master` and `ultra_warm` — so a role outside them publishes no entry: a dedicated
/// coordinator node holds no data and is scaled with the cluster's request load rather
/// than by a published per-type limit, so the catalog reports the role on the instance
/// type ([`instance_roles`]) and no limit block under it.
pub fn instance_type_limits(
    instance_type: &str,
) -> ::std::collections::HashMap<String, Limits> {
    instance_roles(instance_type)
        .into_iter()
        .filter(|role| *role != INSTANCE_ROLE_COORDINATOR)
        .map(|role| {
            let key = if role == INSTANCE_ROLE_ULTRAWARM {
                LIMITS_ROLE_ULTRAWARM
            } else {
                role
            };
            (key.to_string(), role_limits(instance_type, role))
        })
        .collect()
}

/// Whether `ebs` sizes a volume the instance type publishes, and the refusal naming the
/// bounds it publishes otherwise.
///
/// A volume size is one the SERVICE publishes for that instance type and volume
/// sub-type — [`MINIMUM_VOLUME_SIZE_GIB`] up to [`maximum_volume_size`], the
/// `MinimumVolumeSize`/`MaximumVolumeSize` pair `DescribeInstanceTypeLimits` reports
/// for the pairing — so a size outside it asks for storage the service does not offer
/// for that instance type at all. That is a STORAGE LIMIT of the instance type and not
/// a malformed request, so the refusal is the modelled `LimitExceededException`, which
/// the model classifies `client` with HTTP 409 — the class a caller branches on to tell
/// a capacity condition from a bad request.
///
/// The sentence names the bounds, the instance type the volume would attach to and the
/// engine release the domain runs, each taken from the request (or, on an update, from
/// the merged record): the pairing the bounds are published FOR. Options with no volume
/// (EBS not enabled) and options carrying no size at all size nothing, so there is
/// nothing to check; without an instance type there is no instance to size the volume
/// against, and only the floor every instance type publishes applies, so the sentence
/// names that bound alone.
pub fn ebs_volume_size_refusal(
    ebs: Option<&EBSOptions>,
    instance_type: Option<&str>,
    engine_version: &str,
) -> Option<LimitExceededException> {
    let ebs = ebs?;
    if !ebs.ebs_enabled.unwrap_or(false) {
        return None;
    }
    let size = i64::from(ebs.volume_size?);
    // `EBSOptions$VolumeType` "specifies the type of EBS volumes attached to data
    // nodes"; the general-purpose SSD type is what a request that names none gets, and
    // it is not the magnetic type, whose cap is its own.
    let sub_type = ebs.volume_type.as_deref().unwrap_or(VOLUME_TYPE_GP2);
    let maximum = instance_type.map(|instance_type| maximum_volume_size(instance_type, sub_type));
    if size >= MINIMUM_VOLUME_SIZE_GIB && maximum.is_none_or(|maximum| size <= maximum) {
        return None;
    }
    let message = match (instance_type, maximum) {
        (Some(instance_type), Some(maximum)) => format!(
            "Volume size must be between {MINIMUM_VOLUME_SIZE_GIB} and {maximum} for \
             {instance_type} instance type and version {engine_version}"
        ),
        // A request naming no instance type pairs the volume with no instance, so the
        // only bound published for it is the floor, and the sentence names no instance
        // type it has no value for.
        _ => format!(
            "Volume size must be at least {MINIMUM_VOLUME_SIZE_GIB} for version \
             {engine_version}"
        ),
    };
    Some(LimitExceededException {
        message: Some(message),
    })
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

// ---------------------------------------------------------------------------
// Domain configuration a domain cannot be created or updated with
//
// A configuration the SERVICE does not provision is refused rather than stored:
// each check below reads one documented dependency between members of a domain's
// configuration and states the refusal the create and the update paths share, so
// neither path can accept a configuration the other refuses.
// ---------------------------------------------------------------------------

/// The shortest master password the service takes: `MasterUserPassword`'s own
/// `@length` floor, refused at the boundary before any handler runs.
pub const MASTER_USER_PASSWORD_MINIMUM: usize = 8;

/// The character classes a master password has to draw on, stated as the service
/// states them: the sentence a refusal carries, and the list the checks below walk.
///
/// The LENGTH floor is the model's own (`MasterUserPassword` has `@length` min 8), so
/// it is refused at the boundary before any handler runs and is not restated here.
const MASTER_USER_PASSWORD_REQUIREMENT: &str =
    "The master user password must contain at least one uppercase letter, one lowercase letter, \
     one number, and one special character.";

/// Whether `options` carries a master password that meets the service's published
/// complexity requirement, and the refusal stating that requirement otherwise.
///
/// A master user password is the credential of the internal user database, and the
/// service publishes what one has to draw on: an uppercase letter, a lowercase letter,
/// a digit and one character that is none of those — so a password missing any of them
/// is a credential the service would not have accepted, and the request is refused
/// rather than a domain being created or changed around it. A request that carries no
/// password asks for none, so there is nothing to check.
///
/// The refusal states the REQUIREMENT and never the value: `MasterUserPassword` is
/// `@sensitive`, so neither the password nor any part of it is echoed.
pub fn master_user_password_refusal(
    options: Option<&AdvancedSecurityOptionsInput>,
) -> Option<ValidationException> {
    let password = options?
        .master_user_options
        .as_ref()?
        .master_user_password
        .as_deref()?;
    let met = password.chars().any(char::is_uppercase)
        && password.chars().any(char::is_lowercase)
        && password.chars().any(|character| character.is_ascii_digit())
        && password.chars().any(|character| !character.is_alphanumeric());
    if met {
        return None;
    }
    Some(ValidationException {
        message: Some(MASTER_USER_PASSWORD_REQUIREMENT.to_string()),
    })
}

/// Whether `cluster` rests its cold tier on a warm one, and the refusal stating that
/// link otherwise.
///
/// The tiers of a domain form a chain, each level resting on the one below it:
/// `ColdStorageOptions$Enabled` is documented "you must enable UltraWarm storage to
/// enable cold storage". A cluster that asks for the cold tier without the warm one
/// describes a tiering the service cannot provision, so it is refused rather than
/// stored, and the refusal names the link that is missing rather than the chain as a
/// whole: a caller fixes the level it actually left out.
///
/// This link is refused with the operation's `BaseException` ("an error occurred while
/// processing the request", HTTP 400) — the error the service answers it with — and
/// the warm link below with the operation's `ValidationException`, so a caller tells
/// the two apart by code as well as by sentence.
pub fn cold_storage_refusal(cluster: Option<&ClusterConfig>) -> Option<BaseException> {
    let cluster = cluster?;
    let cold = cluster
        .cold_storage_options
        .as_ref()
        .is_some_and(|cold| cold.enabled);
    if !cold || cluster.warm_enabled.unwrap_or(false) {
        return None;
    }
    Some(BaseException {
        message: Some("To use cold storage, you must have UltraWarm enabled.".to_string()),
    })
}

/// Whether `cluster` rests its warm tier on the master nodes that manage it, and the
/// refusal stating that link otherwise.
///
/// The warm nodes of a domain are managed by the cluster's master nodes, so the warm
/// tier is offered only on a cluster whose masters are DEDICATED: a cluster that asks
/// for warm storage without them is refused rather than stored. The next link up is
/// [`cold_storage_refusal`].
pub fn warm_storage_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let cluster = cluster?;
    if !cluster.warm_enabled.unwrap_or(false) || cluster.dedicated_master_enabled.unwrap_or(false) {
        return None;
    }
    Some(ValidationException {
        message: Some(
            "To use warm storage, your domain must have dedicated master nodes.".to_string(),
        ),
    })
}

/// Whether every log-publishing option in `options` names the destination its logs go
/// to, and the refusal naming the first log type that does not otherwise.
///
/// A `LogPublishingOption` is "whether the log should be published" plus "the ARN of
/// the CloudWatch Logs group to publish logs to": an option that is enabled and names
/// no group publishes to nowhere, so it is refused rather than stored as a
/// configuration that silently does nothing. A disabled option publishes nothing and
/// needs no destination.
///
/// The log types are stated in order, so one request with two offending options is
/// refused with the same sentence however the map was iterated.
pub fn log_publishing_refusal<'a>(
    options: impl IntoIterator<Item = (&'a String, &'a LogPublishingOption)>,
) -> Option<ValidationException> {
    let mut offending: Vec<&str> = options
        .into_iter()
        .filter(|(_, option)| {
            option.enabled.unwrap_or(false)
                && option
                    .cloud_watch_logs_log_group_arn
                    .as_deref()
                    .is_none_or(str::is_empty)
        })
        .map(|(log_type, _)| log_type.as_str())
        .collect();
    offending.sort_unstable();
    let log_type = offending.first()?;
    Some(ValidationException {
        message: Some(format!(
            "Log publishing option is misconfigured. Enter valid CloudWatch Logs Arn and Enable \
             flag for LogType \"{log_type}\""
        )),
    })
}

/// Whether `options` leaves the domain's off-peak window enabled, and the refusal
/// naming the restriction otherwise.
///
/// `OffPeakWindowOptions$Enabled` is documented "only available when modifying a
/// domain created prior to February 16, 2023, not when creating a new domain. All
/// domains created after this date have the off-peak window enabled by default. You
/// can't disable the off-peak window after it's enabled for a domain." Every domain
/// this emulator holds is created now, so its window is enabled from the create on and
/// a request that asks for it disabled — at create or at update — is refused rather
/// than leaving a domain with no window for the service's mandatory configuration
/// changes.
pub fn off_peak_window_refusal(
    options: Option<&OffPeakWindowOptions>,
) -> Option<ValidationException> {
    if options?.enabled != Some(false) {
        return None;
    }
    Some(ValidationException {
        message: Some(
            "An off-peak window is required for a domain and cannot be disabled.".to_string(),
        ),
    })
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

/// The `ValidationException` payload for a member naming an engine version the
/// service does not offer: the value exactly as the request carried it, and the
/// catalog it was resolved against — the same set `ListVersions` advertises, so a
/// caller is told which releases it could have asked for and the two catalog answers
/// cannot disagree.
///
/// The catalog is listed in the order it is PUBLISHED in
/// ([`published_engine_versions`], newest release first), which is the order a caller
/// reads it in everywhere else this service hands it over.
pub fn unsupported_engine_version(value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Unsupported Version: {value}, allowed Versions are: [{}]",
            published_engine_versions().join(", ")
        )),
    }
}

/// The engine release `value` names, for an operation that reports what the service
/// offers FOR a named release.
///
/// Two things have to hold, and this is the ONE statement of both: the value has to
/// be readable as a version at all ([`parse_engine_version`]), and it has to resolve
/// against the published catalog ([`offers_engine_version`]). Nothing is offered for
/// a release the service does not have, so a value that resolves to none of them is
/// refused rather than answered for an engine nothing runs.
pub fn resolve_engine_version(
    member: &str,
    value: &str,
) -> Result<EngineVersion, ValidationException> {
    let version =
        parse_engine_version(value).ok_or_else(|| malformed_engine_version(member, value))?;
    if !offers_engine_version(value) {
        return Err(unsupported_engine_version(value));
    }
    Ok(version)
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

/// The modelled refusal for an instance type the engine release in play does not
/// offer: `InvalidTypeException`, "an exception for trying to create or access a
/// sub-resource that's either invalid or not supported", which the model classifies
/// `client` with HTTP 409.
///
/// The sentence names the INSTANCE TYPE alone, taken from the request: the engine
/// release is what the pairing is refused for, not part of the text. This is the ONE
/// construction of it, so every operation that refuses such a pairing words the
/// refusal identically.
pub fn invalid_instance_type(instance_type: &str) -> InvalidTypeException {
    InvalidTypeException {
        message: Some(format!("Invalid instance type: {instance_type}")),
    }
}

/// The refusal for `instance_type` when `version` does not offer it, or `None` when
/// the pairing is one the service offers (and for a request naming no type, which
/// pairs nothing with the release).
///
/// An instance family introduced for a later engine release cannot run an earlier one
/// ([`offers_instance_type`]), so the pairing names a cluster the service cannot
/// provision.
pub fn unoffered_instance_type_refusal(
    version: EngineVersion,
    instance_type: Option<&str>,
) -> Option<InvalidTypeException> {
    let instance_type = instance_type?;
    (!offers_instance_type(version, instance_type)).then(|| invalid_instance_type(instance_type))
}

/// Every instance type `cluster` provisions a node on, in the order the cluster
/// configuration declares them: the data nodes' type, the dedicated master type when
/// dedicated masters are enabled, the warm type when warm storage is enabled, and the
/// type of each enabled node option.
///
/// A type named beside a tier that is turned OFF provisions no node, so it is not a
/// type the cluster runs: `DedicatedMasterType` goes with "indicates whether dedicated
/// master nodes are enabled for the cluster", `WarmType` with "whether to enable warm
/// storage for the cluster", and a `NodeConfig`'s type with "whether a specific node
/// type is active or inactive".
fn provisioned_instance_types(cluster: &ClusterConfig) -> Vec<&str> {
    let mut types: Vec<&str> = Vec::new();
    types.extend(cluster.instance_type.as_deref());
    if cluster.dedicated_master_enabled.unwrap_or(false) {
        types.extend(cluster.dedicated_master_type.as_deref());
    }
    if cluster.warm_enabled.unwrap_or(false) {
        types.extend(cluster.warm_type.as_deref());
    }
    for option in cluster.node_options.iter().flatten() {
        if let Some(config) = option.node_config.as_ref() {
            if config.enabled.unwrap_or(false) {
                types.extend(config.r#type.as_deref());
            }
        }
    }
    types
}

/// The refusal for the first instance type `cluster` provisions a node on that
/// `version` does not offer, or `None` when the release offers every one of them.
///
/// A domain runs ONE engine release, so every node of its cluster runs on an instance
/// type that release offers — the data nodes' type and the type of every other tier
/// alike.
pub fn cluster_instance_type_refusal(
    version: EngineVersion,
    cluster: &ClusterConfig,
) -> Option<InvalidTypeException> {
    provisioned_instance_types(cluster)
        .into_iter()
        .find(|instance_type| !offers_instance_type(version, instance_type))
        .map(invalid_instance_type)
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

/// `PackageStatus`'s `DELETING`: the package is being removed from this account.
///
/// The status a delete leaves the record in, which is the last status any read reports
/// it in: once the clock has taken the record out of the store there is nothing left to
/// report, so `PackageStatus`'s `DELETED` is a status no stored package rests in.
pub const PACKAGE_STATUS_DELETING: &str = "DELETING";

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

/// How many users one package's scope holds before `UpdatePackageScope` refuses to
/// grant another.
///
/// The emulator's OWN bound, like [`PACKAGE_QUOTA`]: the model puts no length on
/// `PackageUserList`, so without one a caller granting fresh accounts could grow a
/// package's stored scope — and the `AllowListedUserList` every read of that package
/// answers with — without end. Every user it counts is released by withdrawing that
/// user's access.
pub const PACKAGE_SCOPE_QUOTA: usize = 100;

/// How many numbered versions of its content one package holds before
/// `UpdatePackage` refuses to put another in place.
///
/// The emulator's OWN bound, like [`PACKAGE_QUOTA`], and not a published service
/// quota: the service documents no limit on how many versions of its content one
/// package may have had. A package's version sequence only ever grows — an import adds
/// a version and nothing removes one — so without a bound a caller looping updates
/// could grow the stored sequence, and the `PackageVersionHistoryList` every read of
/// that history answers with, without end. A package version is a sub-resource of the
/// package, so the bound is the modelled `LimitExceededException` ("trying to create
/// more than the allowed number of resources or sub-resources"), and the whole
/// sequence is released by deleting the package.
pub const PACKAGE_VERSION_QUOTA: usize = 50;

/// One version of a package's content, as this emulator persists it.
///
/// A package's content is imported, and each import is a new numbered version of it,
/// so the versions a package has had are a SEQUENCE and this is one entry of it. It
/// is the ONE record a version is reported through
/// ([`PackageVersionRecord::to_history`]), the way [`InsightRecord`] is for an
/// insight — so the version a read of the package reports and the entry its history
/// lists for that same version cannot disagree.
#[derive(Debug, Clone)]
pub struct PackageVersionRecord {
    /// `PackageVersionHistory$PackageVersion`: which numbered version of the
    /// package's content this entry is, counting from [`FIRST_PACKAGE_VERSION`].
    pub version: u32,
    /// `PackageVersionHistory$CommitMessage`: "a message associated with the package
    /// version when it was uploaded". Absent where the import that added this version
    /// carried none — which includes the first version of every package, because the
    /// create that imports it declares no `CommitMessage` to give.
    pub commit_message: Option<String>,
    /// `PackageVersionHistory$CreatedAt`: "the date and time when the package was
    /// created", for this version of its content.
    pub created_at: SystemTime,
    /// `PackageVersionHistory$PackageConfiguration`: the configuration stored with
    /// this version of the content, absent where the import carried none.
    pub configuration: Option<PackageConfiguration>,
}

impl PackageVersionRecord {
    /// This version as `PackageVersionHistory` — one entry of what
    /// `GetPackageVersionHistory` answers with, so every operation that reports a
    /// package version reports it through the one projection.
    pub fn to_history(&self) -> PackageVersionHistory {
        PackageVersionHistory {
            package_version: Some(self.version.to_string()),
            commit_message: self.commit_message.clone(),
            created_at: Some(datetime(self.created_at)),
            // The plugin's name, class and uncompressed size are read out of the
            // descriptor inside the plugin's own archive, which this emulator does not
            // fetch, so they stay absent rather than being invented — exactly as
            // `PackageDetails$AvailablePluginProperties` does in [`Package::to_details`].
            plugin_properties: None,
            package_configuration: self.configuration.clone(),
        }
    }
}

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
    /// When the clock owes this record's REMOVAL from the account's packages, set by
    /// the delete that asked for it and `None` for every package no delete has
    /// addressed — the package mirror of [`PackageAssociation::removal_due`].
    ///
    /// A status move the record survives is [`Self::owed`]; a removal leaves no record
    /// to carry a status at all, so it is held apart and performed by the store that
    /// holds the record ([`ComAmazonawsOpensearchAmazonopensearchserviceState::settle_packages`]).
    pub removal_due: Option<SystemTime>,
    /// Which numbered version of its content the package currently has IN PLACE —
    /// the version a read reports as available and an association puts on a domain,
    /// counting from [`FIRST_PACKAGE_VERSION`]. A version whose content is still
    /// being copied is not in place yet, so this moves to it when that copy lands.
    pub version: u32,
    /// Every version of this package's content, oldest first — the sequence
    /// `GetPackageVersionHistory` reports. A package is minted with its first version
    /// recorded here and each later import of content appends the next, so the newest
    /// entry is the version [`Self::version`] names once that import's copy has
    /// landed. Bounded by [`PACKAGE_VERSION_QUOTA`].
    pub versions: Vec<PackageVersionRecord>,
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
        // A copy is what puts a version's content in place, so the newest version the
        // history holds becomes the one the package carries once that copy has landed —
        // which is what the move to the available status says has happened.
        if self.status == PACKAGE_STATUS_AVAILABLE {
            if let Some(latest) = self.versions.last() {
                self.version = latest.version;
            }
        }
    }

    /// Start this package's deletion, as the call that asked for it leaves the record.
    ///
    /// The package is not gone the instant that call returns: it is being deleted, so
    /// it rests in [`PACKAGE_STATUS_DELETING`] and the clock owes the record's removal
    /// one [`PROVISIONING`] span later, which
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::settle_packages`] performs
    /// when it falls due. Whatever status move was outstanding is dropped: a package on
    /// its way out is on its way to no other status.
    ///
    /// Idempotent — a package whose removal the clock already owes keeps the instant it
    /// was owed at, so repeating the call cannot push the removal back.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.status = PACKAGE_STATUS_DELETING.to_string();
        self.owed = None;
        self.removal_due = Some(now + PROVISIONING);
        self.updated_at = now;
    }

    /// Whether the removal the clock owed this package has fallen due by `now`, so the
    /// account no longer holds it.
    pub fn removal_landed(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// Record the NEXT numbered version of this package's content, as the call that
    /// put it in place carried it.
    ///
    /// The ONE writer of the package's version sequence, so the numbering is the same
    /// whichever call adds a version: the first content is
    /// [`FIRST_PACKAGE_VERSION`] and each one after it is one past the version before
    /// it, whatever happened to the versions in between. The version this adds is not
    /// in place until its copy lands ([`Self::settle`]), which is why this does not
    /// move [`Self::version`].
    pub fn add_version(
        &mut self,
        commit_message: Option<String>,
        configuration: Option<PackageConfiguration>,
        created_at: SystemTime,
    ) {
        let version = self.versions.last().map_or(FIRST_PACKAGE_VERSION, |latest| {
            latest.version.saturating_add(1)
        });
        self.versions.push(PackageVersionRecord {
            version,
            commit_message,
            created_at,
            configuration,
        });
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

/// `DomainPackageStatus`'s `DISSOCIATION_FAILED`: the removal was attempted and did not
/// happen, so the package is still on the domain — the status an association rests in
/// when the domain cannot let go of the package.
pub const DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED: &str = "DISSOCIATION_FAILED";

/// `ErrorDetails$ErrorMessage` for a removal the domain could not perform because one
/// of its indexes still resolves a file by the package's reference path.
///
/// The service's own fixed sentence for that condition: it carries no value of the
/// request and names neither the package nor the index.
pub const PACKAGE_IN_USE: &str = "Operation not allowed. Package is in use.";

/// The message every refusal of a `PackageID` that resolves to no package of this
/// account carries ([`package_not_found`]).
///
/// The service's own fixed sentence for that condition: it is the WHOLE message and
/// has no variable part, so the id the call could not resolve does not appear in it.
pub const PACKAGE_NOT_FOUND: &str = "Package not found.";

/// `ErrorDetails$ErrorType` for an association whose last transition failed — "the type
/// of error that occurred".
///
/// A transition the domain cannot perform because the package is still being used by
/// one of its indexes is the condition this API models as `ConflictException` ("the
/// client attempts to remove a resource that is currently in use"), so that is the
/// error class the details name.
pub const DOMAIN_PACKAGE_ERROR_TYPE_IN_USE: &str = "ConflictException";

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
    /// When the clock takes this association OFF the domain, for one a dissociation
    /// call put into its removal status; `None` for one no call has asked to remove.
    ///
    /// A removal needs its own due instant rather than an [`OwedStatus`] because
    /// `DomainPackageStatus` has no value for a package that is no longer on the
    /// domain: the record rests in [`DOMAIN_PACKAGE_STATUS_DISSOCIATING`] until the
    /// span has passed and the removal then either takes it off the domain or fails,
    /// leaving it there ([`Domain::settle_package_associations`]).
    pub removal_due: Option<SystemTime>,
    /// `DomainPackageDetails$ErrorDetails`: "additional information if the package is
    /// in an error state. Null otherwise." — so it is populated exactly while this
    /// association's last transition left it in one, and cleared by the next
    /// transition that lands.
    pub error_details: Option<ErrorDetails>,
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
        // A move that LANDED is not an error state: whatever an earlier transition of
        // this association failed at is not this one's.
        self.error_details = None;
    }

    /// Whether this association still has a status move outstanding, meaning the
    /// change asked for by the call that last touched it has not landed yet.
    ///
    /// Read AFTER [`Self::settle`], so a move that has merely fallen due does not
    /// read as one still in flight.
    pub fn in_flight(&self) -> bool {
        self.owed.is_some() || self.removal_due.is_some()
    }

    /// Whether the package's content is in place on the domain for an index's analysis
    /// settings to read a file from.
    ///
    /// The content is there from the moment being put in place has landed until the
    /// record leaves the domain: an association still `ASSOCIATING` has not put it
    /// there yet, and one whose removal failed still has it there, which is what keeps
    /// a dependent index working.
    pub fn in_place(&self) -> bool {
        matches!(
            self.status.as_str(),
            DOMAIN_PACKAGE_STATUS_ACTIVE
                | DOMAIN_PACKAGE_STATUS_DISSOCIATING
                | DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED
        )
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
            // otherwise." — the stored details of the transition that left this
            // association in one, and absent for every association that is not.
            error_details: self.error_details.clone(),
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

/// The refusal every package operation answers a `PackageID` that resolves to no
/// package of this account with.
///
/// The service treats such an id as a bad input VALUE rather than as a resource that
/// is missing: a package that was deleted leaves no record behind, so there is no
/// state for the call to conflict with and nothing about the request will ever make
/// it succeed — which is the client-input refusal (HTTP 400) and not the
/// conflict-shaped missing-resource one (HTTP 409). Its wording is the service's
/// fixed sentence for that condition ([`PACKAGE_NOT_FOUND`]), which carries no value
/// of the request: the rejected id is NOT interpolated into it.
pub fn package_not_found() -> ValidationException {
    ValidationException {
        message: Some(PACKAGE_NOT_FOUND.to_string()),
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

/// The `ValidationException` payload for deleting a package a domain still holds.
///
/// `DeletePackageRequest` states the constraint the delete is judged by — the package
/// "can't be associated with any OpenSearch Service domain" — so a package still in
/// place somewhere is not one this call can remove, and the refusal is the service's
/// fixed sentence for a package that cannot be let go while it is in use
/// ([`PACKAGE_IN_USE`]), which carries no value of the request.
pub fn package_in_use() -> ValidationException {
    ValidationException {
        message: Some(PACKAGE_IN_USE.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for a dissociation of a package the
/// domain is not holding: the link the call asks to remove does not exist, so the
/// refusal names both the package and the domain it is not on.
pub fn package_not_associated(package_id: &str, domain_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Package {package_id} is not associated with domain {domain_name}."
        )),
    }
}

/// The `InternalException` payload for a store that no longer holds what the decision
/// this call already made had resolved — a state the one lock this service takes makes
/// unreachable.
///
/// "Request processing failed because of an unknown error, exception, or internal
/// failure": such a write is not something the CALLER can fix, so it is answered on
/// the server channel rather than reported as a bad request.
pub fn internal_failure(what: &str) -> InternalException {
    InternalException {
        message: Some(format!(
            "Request processing failed: {what} changed while the request was being served."
        )),
    }
}

/// The `ConflictException` payload for an associate or dissociate call made while
/// another change to one of the domain's package associations has not landed yet.
///
/// The service runs these changes on a domain ONE at a time, so the refusal is about
/// the domain rather than about either package, and its wording is the service's own
/// constant for that condition — a fixed sentence every caller that hits it gets,
/// carrying no value of the request.
pub fn concurrent_package_operation() -> ConflictException {
    ConflictException {
        message: Some(
            "Operation not allowed. A concurrent operation (associate/dissociate) is in \
             progress for this domain."
                .to_string(),
        ),
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

/// The `ValidationException` payload for a package-association call whose
/// `DomainName` names no domain this account holds.
///
/// Linking a package resolves the domain as part of READING the request: a name that
/// resolves to no domain is a value the caller has to fix, so the refusal is the
/// client-input one rather than the missing-resource one, and it names the domain
/// that could not be resolved. Compare [`domain_not_found`], which is what a call
/// that ADDRESSES a domain (describing it, changing it, deleting it) answers.
pub fn domain_does_not_exist(domain_name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("The domain {domain_name} does not exist.")),
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

/// What a request asks of ONE association: which package to link to the domain,
/// the packages that must be linked before it, and the configuration to store with
/// the link.
///
/// Both operations that make an association — the one that links a single package
/// and the one that links several at once — describe each link they make with this,
/// so the two cannot ask for different things from the same input.
#[derive(Debug, Clone)]
pub struct AssociationRequest {
    /// The `PackageID` of the package to link, as the request named it.
    pub package_id: String,
    /// The request's `PrerequisitePackageIDList`, empty where it carried none.
    pub prerequisite_package_ids: Vec<String>,
    /// The request's `AssociationConfiguration`, absent where it carried none.
    pub association_configuration: Option<PackageAssociationConfiguration>,
}

/// Why a package cannot be linked to a domain, or unlinked from one, as the
/// modelled exception payload the refusal is reported through.
///
/// Each operation declares its OWN error enum, so this names the payload and the
/// handler lifts it into its own channel: the DECISION lives in one place even
/// though the error channels do not.
#[derive(Debug)]
pub enum AssociationRefusal {
    /// The link the call asks to remove is not there — the domain holds no
    /// association with the package named.
    NotFound(ResourceNotFoundException),
    /// The package or the association is in a status that cannot serve the call.
    Conflict(ConflictException),
    /// The request is not one the domain can serve as asked — a value it carries
    /// (the domain, the package, one of its prerequisites) resolves to nothing, or
    /// the request contradicts itself.
    Validation(ValidationException),
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
    /// operation creates, changes or removes one. The id is the key so a purchase and
    /// the offering-id filter resolve one entry by it; the ORDER a listing publishes the
    /// catalog in is
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::published_reserved_instance_offerings`]'s,
    /// which is what lets a paged read resume where the previous page stopped.
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
    /// The insights held for the ACCOUNT itself — the ones an `Account` entity and
    /// no domain answers for — keyed by their `InsightId`. The insights of a domain
    /// live on that domain ([`Domain::insights`]), so an insight has exactly one
    /// home whichever entity holds it. See [`InsightRecord`].
    pub account_insights: BTreeMap<String, InsightRecord>,
    /// Whether this account may share its packages with other users at all — the
    /// entitlement `UpdatePackageScope` needs before it can grant one.
    ///
    /// ONE account-scoped value rather than a flag per package: it says what the
    /// ACCOUNT may do, so every package of the account is judged by the same value. No
    /// operation of this API carries it and no setting seeds it — a package vendor is
    /// admitted to the programme out of band — so it starts `false`, which is where an
    /// account that has not been admitted stands, and every scope update from such an
    /// account is refused. There is therefore no caller-visible seam that turns it on:
    /// over the wire the refusal is all `UpdatePackageScope` can answer, and the grant
    /// path is reached by a caller holding this store (a test, or a future operation
    /// for an admitted account).
    pub package_vending_entitled: bool,
    /// Every service-managed VPC endpoint of this account, keyed by its
    /// `VpcEndpointId` — unique per endpoint, so the id IS the key.
    /// `CreateVpcEndpoint` mints one and `DescribeVpcEndpoints` / `ListVpcEndpoints`
    /// report it, from THIS one record. Iterating a `BTreeMap` is order-stable, which
    /// is what lets a paged listing resume where the previous page stopped without
    /// repeating or skipping a record. Bounded by [`VPC_ENDPOINT_QUOTA`].
    pub vpc_endpoints: BTreeMap<String, VpcEndpointRecord>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_vpc_endpoint_id`].
    pub vpc_endpoint_seq: u64,
    /// Every OpenSearch UI application of this account, keyed by its `Id` — unique
    /// per application, so the id IS the key. `CreateApplication` mints one and
    /// `GetApplication` reports it, from THIS one record. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads of the set report it in the same
    /// order.
    pub applications: BTreeMap<String, Application>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_application_id`].
    pub application_seq: u64,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_attachment_id`].
    pub attachment_seq: u64,
    /// Every direct query data source of this account, keyed by its
    /// `DataSourceName` — "a unique, user-defined label to identify the data source
    /// within your OpenSearch Service environment", so the name IS the key.
    /// `AddDirectQueryDataSource` mints one and `GetDirectQueryDataSource` reports
    /// it, from THIS one record. Iterating a `BTreeMap` is order-stable, which is
    /// what lets two reads of the set report it in the same order. Bounded by
    /// [`DIRECT_QUERY_DATA_SOURCE_QUOTA`].
    pub direct_query_data_sources: BTreeMap<String, DirectQueryDataSourceRecord>,
    /// Every saved-object migration job of this account, keyed by its `migrationId` —
    /// unique per job, so the id IS the key. `StartMigration` mints one and
    /// `GetMigration` reports it, from THIS one record. Iterating a `BTreeMap` is
    /// order-stable, which is what lets two reads of the set report it in the same
    /// order. Bounded per application by [`MIGRATION_QUOTA`].
    pub migrations: BTreeMap<String, Migration>,
    /// Drives
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_migration_id`].
    pub migration_seq: u64,
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
            account_insights: BTreeMap::new(),
            package_vending_entitled: false,
            vpc_endpoints: BTreeMap::new(),
            vpc_endpoint_seq: 0,
            applications: BTreeMap::new(),
            application_seq: 0,
            attachment_seq: 0,
            direct_query_data_sources: BTreeMap::new(),
            migrations: BTreeMap::new(),
            migration_seq: 0,
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

/// The bijection's constant term for a service-managed VPC endpoint's id. Different
/// from the two above for the same reason: three counters that start at zero must not
/// render the same token.
const VPC_ENDPOINT_ID_OFFSET: u128 = 8_589_934_591;

/// The bijection's constant term for an OpenSearch UI application's id. Different
/// from the three above for the same reason: counters that all start at zero must not
/// render the same token.
const APPLICATION_ID_OFFSET: u128 = 137_438_953_471;

/// The bijection's constant term for a data source attachment's id. Different from the
/// four above for the same reason: counters that all start at zero must not render the
/// same token.
const ATTACHMENT_ID_OFFSET: u128 = 2_199_023_255_551;

/// The bijection's constant term for a saved-object migration job's id. Different from
/// the five above for the same reason: counters that all start at zero must not render
/// the same token.
const MIGRATION_ID_OFFSET: u128 = 35_184_372_088_831;

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

    /// Mint the identifier a service-managed VPC endpoint is addressed by, so two
    /// endpoints never share one.
    ///
    /// `VpcEndpointId` is `@length(min: 5, max: 256)` with pattern
    /// `^aos-[a-zA-Z0-9]*$`: the `aos-` prefix is the service's own, and the body is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], so it is unpredictable-looking yet never
    /// repeats.
    pub fn mint_vpc_endpoint_id(&mut self) -> String {
        self.vpc_endpoint_seq = self.vpc_endpoint_seq.wrapping_add(1);
        format!(
            "aos-{}",
            opaque_token(self.vpc_endpoint_seq, VPC_ENDPOINT_ID_OFFSET)
        )
    }

    /// Mint the identifier an OpenSearch UI application is addressed by, so two
    /// applications never share one.
    ///
    /// `Id` is `^[a-z0-9]{3,30}$` — an opaque lowercase token — so the whole id is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], which renders exactly that alphabet and never
    /// repeats.
    pub fn mint_application_id(&mut self) -> String {
        self.application_seq = self.application_seq.wrapping_add(1);
        opaque_token(self.application_seq, APPLICATION_ID_OFFSET)
    }

    /// Materialise the status move the clock owes every stored application, for a
    /// caller about to report on any of them — and take away the record of every
    /// application whose removal has fallen due.
    pub fn settle_applications(&mut self, now: SystemTime) {
        for application in self.applications.values_mut() {
            application.settle(now);
        }
        self.remove_deleted_applications(now);
    }

    /// Take the record of every application whose removal the clock has made due away,
    /// together with the records held FOR such an application: a saved-object
    /// migration job belongs to the application it migrates into, so a job of an
    /// application the account no longer holds is no longer a job either.
    ///
    /// An application's data source attachments live ON its record, so they go with it
    /// here rather than being hunted down.
    fn remove_deleted_applications(&mut self, now: SystemTime) {
        let removed: Vec<String> = self
            .applications
            .values()
            .filter(|application| application.removal_landed(now))
            .map(|application| application.id.clone())
            .collect();
        for id in &removed {
            self.applications.remove(id);
            self.migrations
                .retain(|_, migration| &migration.application_id != id);
        }
    }

    /// The application a create carrying `client_token` already minted, if this
    /// account holds one: "unique, case-sensitive identifier to ensure idempotency of
    /// the request", so a repeat of the same create is answered with the record the
    /// first one made rather than minting a second application.
    ///
    /// Reads ONLY.
    pub fn application_of_client_token(&self, client_token: &str) -> Option<&Application> {
        self.applications
            .values()
            .find(|application| application.client_token.as_deref() == Some(client_token))
    }

    /// The application `name` names, if this account holds one. An application name is
    /// unique per account and Region, so this is the one lookup a create resolves its
    /// `name` against.
    ///
    /// Reads ONLY.
    pub fn application_named(&self, name: &str) -> Option<&Application> {
        self.applications
            .values()
            .find(|application| application.name == name)
    }

    /// Store `application` and hand back the stored copy, so a create answers with the
    /// record this service persisted rather than a copy beside it.
    ///
    /// The id comes from [`Self::mint_application_id`], a bijection over its own
    /// counter, so the key cannot already be taken; the entry API is used so no path
    /// can overwrite a record this store already holds under one.
    pub fn insert_application(&mut self, application: Application) -> &Application {
        let id = application.id.clone();
        self.applications.entry(id).or_insert(application)
    }

    /// Every application of this account in the order a listing reports them: the order
    /// they were created in, oldest first.
    ///
    /// Two applications created at one instant are ordered by the `Id` they were minted
    /// under, which is unique per application — so the order is TOTAL, and the same set
    /// reports the same order twice however the records are held. That is what lets a
    /// page token name a place in the set.
    ///
    /// Reads ONLY.
    pub fn applications_in_creation_order(&self) -> Vec<&Application> {
        let mut applications: Vec<&Application> = self.applications.values().collect();
        applications.sort_by(|one, other| {
            one.created_at
                .cmp(&other.created_at)
                .then_with(|| one.id.cmp(&other.id))
        });
        applications
    }

    /// Mint the identifier a saved-object migration job is addressed by, so two jobs
    /// never share one.
    ///
    /// `migrationId` is an opaque string the model constrains not at all, so it is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], behind the `mig-` prefix that names the resource
    /// it identifies the way `dsa-` names a data source attachment.
    pub fn mint_migration_id(&mut self) -> String {
        self.migration_seq = self.migration_seq.wrapping_add(1);
        format!(
            "mig-{}",
            opaque_token(self.migration_seq, MIGRATION_ID_OFFSET)
        )
    }

    /// Materialise every status move the clock owes every stored migration job, for a
    /// caller about to report on any of them.
    pub fn settle_migrations(&mut self, now: SystemTime) {
        for migration in self.migrations.values_mut() {
            migration.settle(now);
        }
    }

    /// The migration job `migration_id` names, with every status move the clock owes it
    /// already materialised — the one lookup a read of a single job goes through.
    pub fn settled_migration_mut(
        &mut self,
        migration_id: &str,
        now: SystemTime,
    ) -> Option<&mut Migration> {
        let migration = self.migrations.get_mut(migration_id)?;
        migration.settle(now);
        Some(migration)
    }

    /// The migration job a start carrying `client_token` already minted, if this account
    /// holds one: "if this token matches a previous request, Amazon OpenSearch Service
    /// ignores the request but does not return an error", so a repeat of the same start
    /// is answered with the record the first one made rather than starting a second job.
    ///
    /// Reads ONLY.
    pub fn migration_of_client_token(&self, client_token: &str) -> Option<&Migration> {
        self.migrations
            .values()
            .find(|migration| migration.client_token.as_deref() == Some(client_token))
    }

    /// How many migration jobs this account holds for one application — what
    /// [`MIGRATION_QUOTA`] bounds.
    ///
    /// Counted from the ONE statement of which jobs belong to an application, so the
    /// bound a start is refused by and the set a listing reports are the same set.
    ///
    /// Reads ONLY.
    pub fn migrations_of_application(&self, application_id: &str) -> usize {
        self.migrations_of_application_in_creation_order(application_id)
            .len()
    }

    /// This application's migration jobs in the order a listing reports them: the order
    /// they were started in, oldest first.
    ///
    /// Two jobs started at one instant are ordered by the `migrationId` they were minted
    /// under, which is unique per job — so the order is TOTAL, and the same set reports
    /// the same order twice however the records are held. That is what lets a page token
    /// name a place in the set.
    ///
    /// Reads ONLY.
    pub fn migrations_of_application_in_creation_order(
        &self,
        application_id: &str,
    ) -> Vec<&Migration> {
        let mut migrations: Vec<&Migration> = self
            .migrations
            .values()
            .filter(|migration| migration.application_id == application_id)
            .collect();
        migrations.sort_by(|one, other| {
            one.created_at
                .cmp(&other.created_at)
                .then_with(|| one.migration_id.cmp(&other.migration_id))
        });
        migrations
    }

    /// Store `migration` and hand back the stored copy, so a start answers with the
    /// record this service persisted rather than a copy beside it.
    ///
    /// The id comes from [`Self::mint_migration_id`], a bijection over its own counter,
    /// so the key cannot already be taken; the entry API is used so no path can
    /// overwrite a record this store already holds under one.
    pub fn insert_migration(&mut self, migration: Migration) -> &Migration {
        let id = migration.migration_id.clone();
        self.migrations.entry(id).or_insert(migration)
    }

    /// The application `id` names, with any status move the clock owes it already
    /// materialised — the one lookup a read of a single application goes through.
    ///
    /// An application whose removal has fallen due names nothing: its record is taken
    /// away here, so a read of it finds no such application rather than one resting in
    /// its deleting status for ever.
    pub fn settled_application_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut Application> {
        let application = self.applications.get_mut(id)?;
        application.settle(now);
        if application.removal_landed(now) {
            self.remove_deleted_applications(now);
            return None;
        }
        self.applications.get_mut(id)
    }

    /// Mint the identifier a data source attachment is addressed by, so two
    /// attachments never share one.
    ///
    /// `attachmentId` is an opaque string the model constrains not at all, so it is
    /// minted off its own counter through the same bijection as
    /// [`Self::mint_endpoint_suffix`], behind the `dsa-` prefix that names the resource
    /// it identifies the way `cc-` names a cross-cluster connection.
    pub fn mint_attachment_id(&mut self) -> String {
        self.attachment_seq = self.attachment_seq.wrapping_add(1);
        format!(
            "dsa-{}",
            opaque_token(self.attachment_seq, ATTACHMENT_ID_OFFSET)
        )
    }

    /// Materialise the status move the clock owes every data source attachment of every
    /// stored application, for a caller about to report on any of them.
    ///
    /// The data sources the account still holds are resolved FIRST, because that is
    /// what decides whether a pending attachment that has fallen due completes or
    /// fails: an attachment whose data source is gone can never complete. Each DISTINCT
    /// ARN the attachments name is resolved once, so two applications attached to one
    /// data source cost one resolution rather than two.
    pub fn settle_attachments(&mut self, now: SystemTime) {
        let mut data_sources: BTreeSet<String> = self
            .applications
            .values()
            .flat_map(|application| application.attachments.keys())
            .cloned()
            .collect();
        data_sources.retain(|arn| self.holds_data_source(arn));
        for application in self.applications.values_mut() {
            for attachment in application.attachments.values_mut() {
                attachment.settle(now, &data_sources);
            }
        }
    }

    /// Materialise everything the clock owes the records an attachment operation reads:
    /// the applications the attachments belong to, and the attachments' own moves.
    ///
    /// The ONE place the attachment operations settle, so the four of them cannot drift
    /// apart over which records they report at `now`.
    pub fn settle_for_attachments(&mut self, now: SystemTime) {
        self.settle_applications(now);
        self.settle_attachments(now);
    }

    /// The domain `arn` names, if this account holds one — how an operation that takes
    /// a domain's ARN rather than its name resolves it.
    ///
    /// Reads ONLY.
    pub fn domain_of_arn(&self, arn: &str) -> Option<&Domain> {
        self.domains.values().find(|domain| domain.arn == arn)
    }

    /// When the data source `arn` names becomes active, and whether this account holds
    /// one under that ARN at all — see [`DataSourceActivation`].
    ///
    /// An application's data source is any of the kinds this service attaches to one, so
    /// the ARN is resolved against every kind the account can hold rather than against
    /// its domains alone: a domain of this account, a direct query data source of it, or
    /// this account's AWS Glue Data Catalog. Anything else names no data source of this
    /// account, which is what an attachment can never complete against.
    ///
    /// The ONE resolution of a data source ARN, so the attach that decides an
    /// attachment's outcome and the settlement that lands it cannot disagree about what
    /// the account holds.
    ///
    /// Reads ONLY.
    pub fn data_source_activation(&self, arn: &str) -> DataSourceActivation {
        // A domain is provisioned, so the state of the domain the ARN names is what
        // decides whether its data is reachable yet.
        if let Some(domain) = self.domain_of_arn(arn) {
            return match domain.cluster_up_at() {
                Some(due) => DataSourceActivation::ActiveAt(due),
                None => DataSourceActivation::Active,
            };
        }
        // A direct query data source is queryable from the moment the add that minted it
        // answered: nothing is provisioned for it and no status moves on it.
        if self
            .direct_query_data_sources
            .values()
            .any(|held| held.arn == arn)
        {
            return DataSourceActivation::Active;
        }
        // The account's Data Catalog is not a resource anything provisions, so it is
        // active whenever it is named.
        if is_account_glue_data_catalog_arn(arn) {
            return DataSourceActivation::Active;
        }
        DataSourceActivation::Absent
    }

    /// Whether this account holds a data source under `arn` — what decides whether a
    /// pending attachment that has fallen due completes or fails.
    pub fn holds_data_source(&self, arn: &str) -> bool {
        self.data_source_activation(arn) != DataSourceActivation::Absent
    }

    /// Materialise the status move the clock owes every stored VPC endpoint, for a
    /// caller about to report on any of them.
    pub fn settle_vpc_endpoints(&mut self, now: SystemTime) {
        for endpoint in self.vpc_endpoints.values_mut() {
            endpoint.settle(now);
        }
    }

    /// Give `domain` the opaque host id its VPC endpoints are named by, if it is a
    /// VPC-access domain that has none yet.
    ///
    /// The ONE place that decides a domain needs one, so a domain reaches it however
    /// it came to hold `VPCOptions` — created with them, or given them by a later
    /// update — and a domain that already holds one keeps it, which is what makes the
    /// hosts [`Domain::vpc_access_endpoints`] publishes stable across calls. A domain
    /// with no `VPCOptions` is reached on its public endpoint and gets none.
    pub fn provision_vpc_access_host(&mut self, domain: &mut Domain) {
        if domain.vpc_options.is_some() && domain.vpc_access_suffix.is_none() {
            domain.vpc_access_suffix = Some(self.mint_endpoint_suffix());
        }
    }

    /// The domain a `CreateVpcEndpoint` can mint an endpoint for, given the
    /// `DomainArn` the request named.
    ///
    /// TWO conditions, and the refusal does not say which failed
    /// ([`vpc_endpoint_domain_unavailable`]): the ARN must name a domain this account
    /// holds, and that domain must be one an endpoint can be created for — a domain
    /// provisioned inside a VPC, because the endpoint exists to reach a cluster that
    /// has no public endpoint of its own. A domain with no `VPCOptions` has nothing
    /// for an endpoint to proxy into.
    ///
    /// Reads ONLY.
    pub fn vpc_endpoint_domain(&self, domain_arn: &str) -> Option<&Domain> {
        self.domains
            .values()
            .find(|domain| domain.arn == domain_arn && domain.vpc_options.is_some())
    }

    /// The endpoint a create carrying `client_token` already minted, if this account
    /// holds one.
    ///
    /// `ClientToken` is "a unique, case-sensitive identifier to ensure idempotency of
    /// the request", so a repeat of the same create is answered with the record the
    /// first one made rather than minting a second endpoint.
    ///
    /// Reads ONLY.
    pub fn vpc_endpoint_of_client_token(&self, client_token: &str) -> Option<&VpcEndpointRecord> {
        self.vpc_endpoints
            .values()
            .find(|endpoint| endpoint.client_token.as_deref() == Some(client_token))
    }

    /// Store `endpoint` and hand back the stored copy, so a create answers with the
    /// record this service persisted rather than a copy beside it.
    pub fn insert_vpc_endpoint(&mut self, endpoint: VpcEndpointRecord) -> &VpcEndpointRecord {
        let id = endpoint.vpc_endpoint_id.clone();
        self.vpc_endpoints.entry(id).or_insert(endpoint)
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

    /// Keep the records of deleted connections within [`CONNECTION_HISTORY`],
    /// releasing the oldest first.
    ///
    /// A deleted connection's record stays addressable rather than being taken out on
    /// a timer, so what bounds the store is a COUNT: past the cap the oldest deleted
    /// record is released, which is the only thing that releases one. Called where the
    /// store grows — a create — so the cap is applied before another record is added
    /// and the set of records cannot grow with the number of creates and deletes a
    /// caller makes.
    pub fn trim_connection_history(&mut self) {
        let mut deleted: Vec<(u64, String)> = self
            .connections
            .values()
            .filter(|connection| connection.deleted())
            .map(|connection| (connection.sequence, connection.connection_id.clone()))
            .collect();
        if deleted.len() <= CONNECTION_HISTORY {
            return;
        }
        deleted.sort_unstable();
        let over = deleted.len() - CONNECTION_HISTORY;
        for (_, connection_id) in deleted.into_iter().take(over) {
            self.connections.remove(&connection_id);
        }
    }

    /// The cross-cluster connection `connection_id` names, with every status move the
    /// clock owes it already materialised — so an operation on one connection decides
    /// from the status the record is in at `now`, exactly as a report of the whole set
    /// would have shown it.
    ///
    /// `None` only where the id addresses no record at all: a connection that was
    /// deleted comes to rest in [`CONNECTION_STATUS_DELETED`] and its id goes on
    /// resolving to that record, so an id this service minted for this account stops
    /// resolving only once the store has released the record
    /// ([`Self::trim_connection_history`]).
    pub fn settled_connection_mut(
        &mut self,
        connection_id: &str,
        now: SystemTime,
    ) -> Option<&mut Connection> {
        let connection = self.connections.get_mut(connection_id)?;
        connection.settle(now);
        Some(connection)
    }

    /// Materialise the status move the clock owes every stored package, for a caller
    /// about to report on all of them — and release the record of every package whose
    /// deletion has fallen due.
    pub fn settle_packages(&mut self, now: SystemTime) {
        for package in self.packages.values_mut() {
            package.settle(now);
        }
        // A package the clock owed a removal is no longer one the account holds, so no
        // read reports it again and the quota slot it counted is free. Done here rather
        // than in [`Package::settle`] because a record cannot take itself out of the
        // map that holds it.
        self.packages.retain(|_, package| !package.removal_landed(now));
    }

    /// The insights the entity a request names holds.
    ///
    /// "An entity can be an Amazon OpenSearch Service domain or an Amazon Web
    /// Services account": a `DomainName` entity holds the insights of that one
    /// domain, and an `Account` entity holds the account's insights — its own and
    /// those of every domain it holds, because an insight about a domain is an
    /// insight of the account the domain belongs to.
    ///
    /// Reads ONLY, and refuses rather than reporting an empty set where the entity
    /// names nothing this account holds, so every operation that resolves an
    /// `InsightEntity` resolves it the same way.
    pub fn insights_of_entity(
        &self,
        entity: &InsightEntity,
    ) -> Result<Vec<&InsightRecord>, InsightEntityRefusal> {
        match entity.r#type.as_str() {
            INSIGHT_ENTITY_DOMAIN_NAME => {
                // "For `DomainName`, this is the domain name": an entity of this type
                // names its domain in `Value`, so one that names none resolves to no
                // entity at all rather than to every domain.
                let name = insight_entity_value(entity)
                    .ok_or_else(|| InsightEntityRefusal::Validation(insight_value_required()))?;
                let domain = self
                    .domains
                    .get(name)
                    .ok_or_else(|| InsightEntityRefusal::NotFound(domain_not_found(name)))?;
                Ok(domain.insights.values().collect())
            }
            INSIGHT_ENTITY_ACCOUNT => {
                // "For `Account`, this is the Amazon Web Services account ID": the
                // only account whose insights this emulator holds is the one its
                // requests are made for, so any other account id names an entity it
                // cannot report on.
                if let Some(value) = insight_entity_value(entity) {
                    if value != ::engine::emulator_account_id() {
                        return Err(InsightEntityRefusal::Validation(foreign_account_entity(
                            value,
                        )));
                    }
                }
                Ok(self
                    .account_insights
                    .values()
                    .chain(
                        self.domains
                            .values()
                            .flat_map(|domain| domain.insights.values()),
                    )
                    .collect())
            }
            other => Err(InsightEntityRefusal::Validation(unknown_insight_entity(
                other,
            ))),
        }
    }

    /// The one insight the entity a request names holds under `insight_id`, or
    /// `None` where that entity holds none under it.
    ///
    /// An insight is addressed by its entity and its id together, so this resolves
    /// the entity exactly as [`Self::insights_of_entity`] does — the same ONE
    /// decision, and the only one that can refuse — and then looks the id up among
    /// the insights THAT entity holds, so an insight another entity holds is never
    /// reported here. An id the entity holds no insight under is not a refusal: it
    /// names no insight of this entity, which the caller reports as it reports any
    /// other id it resolves nothing for.
    ///
    /// Reads ONLY.
    pub fn insight_of_entity(
        &self,
        entity: &InsightEntity,
        insight_id: &str,
    ) -> Result<Option<&InsightRecord>, InsightEntityRefusal> {
        Ok(self
            .insights_of_entity(entity)?
            .into_iter()
            .find(|record| record.insight_id == insight_id))
    }

    /// The one insight a feedback submission addresses, ready to be written.
    ///
    /// Feedback "submits feedback for an EXISTING insight in an Amazon OpenSearch
    /// Service domain", and `InsightFeedbackEntityType` admits `DomainName` alone, so
    /// a submission is addressed by the domain that holds the insight and the
    /// insight's own id together: BOTH have to resolve, because a domain this account
    /// does not hold and an id that domain holds no insight under alike leave no
    /// insight for the rating to be about. Which of the two failed to resolve is not
    /// reported — the submission named a resource this service cannot resolve either
    /// way ([`insight_resource_not_found`]) — while an entity whose type is outside
    /// the enum names no kind of resource at all and is refused as a malformed
    /// request.
    pub fn insight_for_feedback_mut(
        &mut self,
        entity: &InsightFeedbackEntity,
        insight_id: &str,
    ) -> Result<&mut InsightRecord, InsightEntityRefusal> {
        if entity.r#type != INSIGHT_ENTITY_DOMAIN_NAME {
            return Err(InsightEntityRefusal::Validation(
                unknown_insight_feedback_entity(&entity.r#type),
            ));
        }
        // "The value of the entity, such as a domain name": an entity of this type
        // names its domain in `Value`, and `Value` is `@required` here, so one that
        // names no name at all resolves to no domain rather than to every domain.
        let name = entity.value.trim();
        self.domains
            .get_mut(name)
            .and_then(|domain| domain.insights.get_mut(insight_id))
            .ok_or_else(|| InsightEntityRefusal::NotFound(insight_resource_not_found()))
    }

    /// The tag set of the resource `arn` names, for a caller about to READ it.
    ///
    /// A domain, an application and a direct query data source are the taggable
    /// resources this store persists an ARN for ([`domain_arn`], [`application_arn`],
    /// [`direct_query_data_source_arn`]); the data source of a DOMAIN that the tagging
    /// operations also name has no stored ARN to resolve against (it is addressed by
    /// its domain and name), so an ARN naming none of them resolves to nothing and the
    /// caller refuses it.
    pub fn tagged_resource(&self, arn: &str) -> Option<&TagSet> {
        self.domains
            .values()
            .find(|domain| domain.arn == arn)
            .map(|domain| &domain.tags)
            .or_else(|| {
                self.applications
                    .values()
                    .find(|application| application.arn == arn)
                    .map(|application| &application.tags)
            })
            .or_else(|| {
                self.direct_query_data_sources
                    .values()
                    .find(|data_source| data_source.arn == arn)
                    .map(|data_source| &data_source.tags)
            })
    }

    /// The tag set of the resource `arn` names, for a caller about to WRITE it —
    /// resolved exactly as [`Self::tagged_resource`] resolves it, so a tag write and
    /// a tag read can never disagree about which resource an ARN names.
    pub fn tagged_resource_mut(&mut self, arn: &str) -> Option<&mut TagSet> {
        if let Some(domain) = self.domains.values_mut().find(|domain| domain.arn == arn) {
            return Some(&mut domain.tags);
        }
        if let Some(application) = self
            .applications
            .values_mut()
            .find(|application| application.arn == arn)
        {
            return Some(&mut application.tags);
        }
        self.direct_query_data_sources
            .values_mut()
            .find(|data_source| data_source.arn == arn)
            .map(|data_source| &mut data_source.tags)
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

    /// Whether any domain of this account holds the package `package_id` — the
    /// condition that keeps the package from being deleted.
    ///
    /// A link is held ON the domain under the package's id, so this is the same set
    /// `ListDomainsForPackage` reports: an association the clock has not taken off the
    /// domain yet still counts, because the package is still in place there.
    ///
    /// Reads ONLY, and is read AFTER [`Domain::settle_package_associations`], so a link
    /// whose removal has already fallen due is not counted.
    pub fn package_is_associated(&self, package_id: &str) -> bool {
        self.domains
            .values()
            .any(|domain| domain.package_associations.contains_key(package_id))
    }

    /// Decide whether `request` can be served on `domain_name` and, where it can,
    /// hand back the stored [`Package`] the association will link to.
    ///
    /// Reads ONLY: this is the whole decision for one link, so a caller making
    /// several of them can decide on all of them before writing any — which is what
    /// lets a refused request leave the domain's associations exactly as they were.
    ///
    /// `associated_by_request` names the packages the SAME request associates, which
    /// is what satisfies a prerequisite "linked to the domain before or
    /// simultaneously with the package association"; a request that links a single
    /// package passes an empty set, because such a call associates nothing else.
    ///
    /// Call AFTER [`Self::settle_packages`] and the domain's own settling, so a
    /// package whose copy has finished and an association whose move has landed are
    /// judged in the status the clock already left them in.
    pub fn plan_package_association(
        &self,
        domain_name: &str,
        request: &AssociationRequest,
        associated_by_request: &BTreeSet<String>,
    ) -> Result<Package, AssociationRefusal> {
        let domain = self
            .domains
            .get(domain_name)
            .ok_or_else(|| AssociationRefusal::Validation(domain_does_not_exist(domain_name)))?;
        let package = self
            .packages
            .get(&request.package_id)
            .ok_or_else(|| AssociationRefusal::Validation(package_not_found()))?;

        // A package can be put on a domain only once its content is in place: until
        // then there is no version of it to associate, which is the conflict between
        // the call and the status the package is in.
        if package.status != PACKAGE_STATUS_AVAILABLE {
            return Err(AssociationRefusal::Conflict(package_not_available(
                &request.package_id,
                &package.status,
            )));
        }

        // The service runs one change to a domain's package associations at a time: a
        // call made while another associate or dissociate on the SAME domain has not
        // landed yet is refused rather than overtaking it. Once every change has
        // landed, a package already on the domain is re-associated, which is what
        // moves the domain onto a newer version of its content.
        if domain
            .package_associations
            .values()
            .any(PackageAssociation::in_flight)
        {
            return Err(AssociationRefusal::Conflict(concurrent_package_operation()));
        }

        for prerequisite in &request.prerequisite_package_ids {
            if prerequisite == &request.package_id {
                return Err(AssociationRefusal::Validation(prerequisite_is_self(
                    &request.package_id,
                )));
            }
            if !self.packages.contains_key(prerequisite) {
                return Err(AssociationRefusal::Validation(package_not_found()));
            }
            // A prerequisite "must be linked to the domain before or simultaneously
            // with the package association": one the domain already holds satisfies
            // the first, one the same call associates the second, and anything else
            // is a request the domain cannot serve as asked.
            let satisfied = domain.package_associations.contains_key(prerequisite)
                || associated_by_request.contains(prerequisite);
            if !satisfied {
                return Err(AssociationRefusal::Validation(prerequisite_not_associated(
                    &request.package_id,
                    prerequisite,
                    domain_name,
                )));
            }
        }

        Ok(package.clone())
    }

    /// Make the association `request` asks for on `domain_name` and hand back the
    /// stored record as `DomainPackageDetails`.
    ///
    /// `package` is the record [`Self::plan_package_association`] resolved, so this
    /// writes only a link the decision above admitted. `None` only where the domain
    /// is gone, which the plan already refused.
    ///
    /// The association REPLACES any the domain already had with the same package: a
    /// domain holds one per package, which is also what makes a second call the way a
    /// newer version of the package's content reaches the domain.
    pub fn put_package_association(
        &mut self,
        domain_name: &str,
        package: &Package,
        request: &AssociationRequest,
        now: SystemTime,
    ) -> Option<DomainPackageDetails> {
        let domain = self.domains.get_mut(domain_name)?;
        let association = PackageAssociation {
            package_id: package.package_id.clone(),
            // The package is not on the domain the instant the call returns: it is
            // being put there, and the clock owes the association the status that
            // leaves it in once the span has passed.
            status: DOMAIN_PACKAGE_STATUS_ASSOCIATING.to_string(),
            owed: Some(OwedStatus {
                status: DOMAIN_PACKAGE_STATUS_ACTIVE.to_string(),
                message: None,
                due: now + PACKAGE_ASSOCIATION_CHANGE,
            }),
            // The version this association puts on the domain is the one the package
            // has available now, so a package whose content was updated reaches the
            // domain through THIS call rather than on its own.
            package_version: package.version.to_string(),
            // The call's own list, which REPLACES whatever an earlier association of
            // the same package recorded: the prerequisites belong to the association
            // this call makes.
            prerequisite_package_ids: request.prerequisite_package_ids.clone(),
            association_configuration: request.association_configuration.clone(),
            last_updated: now,
            // This call puts the package ON the domain, so nothing about it is owed a
            // removal: a dissociation is what asks for one.
            removal_due: None,
            // The package is being put in place, which is not an error state; whatever
            // an earlier association of the same package failed at is not this one's.
            error_details: None,
        };
        // Answered from the STORED record, through the projection every read path
        // reports an association with: the slot the write went into IS the record, so
        // there is no second lookup and no second way this can fail.
        let stored = match domain.package_associations.entry(package.package_id.clone()) {
            Entry::Occupied(mut slot) => {
                slot.insert(association);
                slot.into_mut()
            }
            Entry::Vacant(slot) => slot.insert(association),
        };
        Some(stored.to_details(domain_name, package))
    }

    /// Decide whether the association between `domain_name` and `package_id` can be
    /// removed and, where it can, hand back the stored [`Package`] it links to.
    ///
    /// Reads ONLY — the mirror of [`Self::plan_package_association`], so a call that
    /// removes several links can decide on all of them before writing any, and a
    /// refused request leaves the domain's associations exactly as they were.
    ///
    /// Call AFTER [`Self::settle_packages`] and the domain's own settling, so an
    /// association whose move has landed is judged in the status the clock already
    /// left it in, and one whose removal has already fallen due is no longer there to
    /// be removed again.
    pub fn plan_package_dissociation(
        &self,
        domain_name: &str,
        package_id: &str,
    ) -> Result<Package, AssociationRefusal> {
        let domain = self
            .domains
            .get(domain_name)
            .ok_or_else(|| AssociationRefusal::Validation(domain_does_not_exist(domain_name)))?;
        let package = self
            .packages
            .get(package_id)
            .ok_or_else(|| AssociationRefusal::Validation(package_not_found()))?;

        // The service runs one change to a domain's package associations at a time, so
        // a call made while another associate or dissociate on the SAME domain has not
        // landed yet is refused rather than overtaking it — the identical rule the
        // associate decision applies, because it is one rule about the domain.
        //
        // It is judged BEFORE the link itself, in the same place the associate decision
        // judges it: the condition is about the DOMAIN, so which of its associations
        // the call happens to name cannot change the answer — a domain with a change
        // still landing refuses every package call alike, and only once nothing is in
        // flight does the set it holds decide the call.
        if domain
            .package_associations
            .values()
            .any(PackageAssociation::in_flight)
        {
            return Err(AssociationRefusal::Conflict(concurrent_package_operation()));
        }

        // A removal needs a link to remove: a package the domain is not holding is
        // nothing this call can take off it.
        if !domain.package_associations.contains_key(package_id) {
            return Err(AssociationRefusal::NotFound(package_not_associated(
                package_id,
                domain_name,
            )));
        }

        Ok(package.clone())
    }

    /// Take the package off `domain_name` and hand back the stored record as
    /// `DomainPackageDetails`.
    ///
    /// `package` is the record [`Self::plan_package_dissociation`] resolved, so this
    /// writes only a removal the decision above admitted. `None` only where the
    /// domain or the association is gone, which that plan already refused.
    ///
    /// The package is still on the domain the instant the call returns: it is being
    /// taken off, so the association rests in [`DOMAIN_PACKAGE_STATUS_DISSOCIATING`]
    /// and the clock owes it the OUTCOME of that removal
    /// ([`PACKAGE_ASSOCIATION_CHANGE`]) — which is decided when it falls due, from the
    /// indexes the domain holds then ([`Domain::settle_package_associations`]), not
    /// here: an index created while the removal is in flight still depends on the
    /// package by the time the removal would happen.
    pub fn remove_package_association(
        &mut self,
        domain_name: &str,
        package: &Package,
        now: SystemTime,
    ) -> Option<DomainPackageDetails> {
        let domain = self.domains.get_mut(domain_name)?;
        let association = domain.package_associations.get_mut(&package.package_id)?;
        association.status = DOMAIN_PACKAGE_STATUS_DISSOCIATING.to_string();
        // Whatever move was outstanding is replaced by the removal this call asked for:
        // the association is on its way off the domain, not on its way to any other
        // status, and it is not in an error state while that is in flight.
        association.owed = None;
        association.removal_due = Some(now + PACKAGE_ASSOCIATION_CHANGE);
        association.error_details = None;
        association.last_updated = now;
        // Answered from the STORED record, through the projection every read path
        // reports an association with.
        Some(association.to_details(domain_name, package))
    }

    /// The published offering catalog, in the ORDER the service publishes it
    /// ([`offering_catalog_rank`]).
    ///
    /// The store is keyed by offering id so a purchase and the id filter can resolve one
    /// entry, and that key orders the catalog by a digest rather than by anything a
    /// caller reads — so the listing is published along the catalog's own order instead.
    /// The order is a function of the records alone, so two reads report the same
    /// offerings in the same order and a page resumes where the previous one stopped.
    pub fn published_reserved_instance_offerings(&self) -> Vec<&ReservedInstanceOffering> {
        let mut catalog: Vec<&ReservedInstanceOffering> =
            self.reserved_instance_offerings.values().collect();
        catalog.sort_by_key(|offering| offering_catalog_rank(offering));
        catalog
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

/// `IamIdentityCenterOptionsInput` as the service reports it back.
///
/// The same reasoning as [`stored_identity_center_options`]: the IAM Identity Center
/// application's own ARN is minted by Identity Center itself, which this emulator does
/// not model, so it stays absent rather than being invented.
pub fn stored_iam_identity_center_options(
    input: IamIdentityCenterOptionsInput,
) -> IamIdentityCenterOptions {
    IamIdentityCenterOptions {
        enabled: input.enabled,
        iam_identity_center_instance_arn: input.iam_identity_center_instance_arn,
        iam_role_for_identity_center_application_arn: input
            .iam_role_for_identity_center_application_arn,
        iam_identity_center_application_arn: None,
    }
}

/// `IamIdentityCenterOptionsInput` as an UPDATE leaves the settings a record already
/// carries.
///
/// The same rule an update applies to the members of the request, one level down: a
/// member the request carries replaces what the record held, and a member it leaves
/// out keeps the stored value. The Identity Center application's own ARN is minted by
/// that service rather than by a caller, so an update never moves it — the same
/// reasoning as [`stored_iam_identity_center_options`].
pub fn updated_iam_identity_center_options(
    stored: Option<IamIdentityCenterOptions>,
    input: IamIdentityCenterOptionsInput,
) -> IamIdentityCenterOptions {
    let mut options = stored.unwrap_or_default();
    if input.enabled.is_some() {
        options.enabled = input.enabled;
    }
    if input.iam_identity_center_instance_arn.is_some() {
        options.iam_identity_center_instance_arn = input.iam_identity_center_instance_arn;
    }
    if input
        .iam_role_for_identity_center_application_arn
        .is_some()
    {
        options.iam_role_for_identity_center_application_arn =
            input.iam_role_for_identity_center_application_arn;
    }
    options
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

/// `UpgradeDomainResponse$UpgradeId`: "the unique identifier of the domain upgrade",
/// a `GUID` like every other id this service hands out ([`new_guid`]), so no two
/// runs share one.
pub fn new_upgrade_id() -> String {
    new_guid()
}

/// `DryRunProgressStatus$DryRunId`: "the unique identifier of the dry run", a `GUID`
/// like every other id this service hands out ([`new_guid`]), so no two runs share
/// one.
fn new_dry_run_id() -> String {
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

/// A schema document mirroring the JSON value a REST request carried.
///
/// An index's schema is stored as the modelled document `CreateIndex` carries
/// ([`Index::schema`]), while the domain's own REST data plane reads and writes JSON, so
/// the two meet here — one index record whichever path created it, rather than two stores
/// that could report the same index differently.
#[must_use]
pub fn document_from_json(value: &::serde_json::Value) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::{Document, Number};
    match value {
        ::serde_json::Value::Null => Document::Null,
        ::serde_json::Value::Bool(flag) => Document::Bool(*flag),
        ::serde_json::Value::String(text) => Document::String(text.clone()),
        ::serde_json::Value::Number(number) => Document::Number(
            number
                .as_i64()
                .map(Number::NegInt)
                .unwrap_or_else(|| Number::Float(number.as_f64().unwrap_or_default())),
        ),
        ::serde_json::Value::Array(elements) => {
            Document::Array(elements.iter().map(document_from_json).collect())
        }
        ::serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), document_from_json(member)))
                .collect(),
        ),
    }
}

/// [`document_from_json`] the other way: the JSON value a stored schema document is
/// reported as on the REST data plane.
#[must_use]
pub fn json_from_document(document: &::aws_smithy_types::Document) -> ::serde_json::Value {
    use ::aws_smithy_types::{Document, Number};
    match document {
        Document::Null => ::serde_json::Value::Null,
        Document::Bool(flag) => ::serde_json::Value::Bool(*flag),
        Document::String(text) => ::serde_json::Value::String(text.clone()),
        Document::Number(Number::PosInt(value)) => ::serde_json::Value::from(*value),
        Document::Number(Number::NegInt(value)) => ::serde_json::Value::from(*value),
        Document::Number(Number::Float(value)) => ::serde_json::Number::from_f64(*value)
            .map(::serde_json::Value::Number)
            .unwrap_or(::serde_json::Value::Null),
        Document::Array(elements) => {
            ::serde_json::Value::Array(elements.iter().map(json_from_document).collect())
        }
        Document::Object(members) => ::serde_json::Value::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), json_from_document(member)))
                .collect(),
        ),
    }
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

/// The host a VPC-access domain is reached on over IPv4 — what
/// `DomainStatus$Endpoints` publishes under [`ENDPOINTS_KEY_VPC`].
///
/// The shape is that member's own example,
/// `vpc-endpoint-h2dsd34efgyghrtguk5gt6j2foh4.us-east-1.es.amazonaws.com`: the
/// `vpc-endpoint-` prefix, an opaque per-domain id, then the Region and this service's
/// endpoint prefix. Unlike [`domain_endpoint`] it carries no domain name: the host is
/// reached from inside the VPC and names only the endpoint.
pub fn vpc_access_endpoint(suffix: &str, region: &str) -> String {
    format!("vpc-endpoint-{suffix}.{region}.{ARN_NAMESPACE}.amazonaws.com")
}

/// The host a dual-stack VPC-access domain is reached on over IPv4 *or* IPv6 — what
/// `DomainStatus$Endpoints` publishes under [`ENDPOINTS_KEY_VPC_DUALSTACK`], beside
/// the IPv4 host of [`vpc_access_endpoint`].
///
/// The shape is that member's own example,
/// `vpc-endpoint-h2dsd34efgyghrtguk5gt6j2foh4.aos.us-east-1.on.aws`: the SAME opaque
/// id as the IPv4 host, under the service's dual-stack domain.
pub fn vpc_access_endpoint_v2(suffix: &str, region: &str) -> String {
    dual_stack_host(&format!("vpc-endpoint-{suffix}"), region)
}

/// One host of this service's DUAL-STACK domain: the label a domain is reached under over
/// IPv4, under `aos.<region>.on.aws` instead of the Region's `es` endpoint prefix.
///
/// The dual-stack domain is the one `DomainStatus$Endpoints`' own `vpcv2` example names
/// (`vpc-endpoint-<opaque-id>.aos.<region>.on.aws`), and the label is the IPv4 host's
/// because the version 2 endpoint IS that endpoint: "it functions like a normal endpoint,
/// except that it works with both IPv4 and IPv6 IP addresses"
/// (`DomainStatus$EndpointV2`). The one place that knows that domain, so the host a
/// VPC-access domain publishes in its map and the one a public domain publishes as its
/// `EndpointV2` cannot be built differently.
fn dual_stack_host(label: &str, region: &str) -> String {
    format!("{label}.aos.{region}.on.aws")
}

/// The connection endpoint a service-managed VPC endpoint is reached on.
///
/// The same host shape as [`domain_endpoint`] — the domain, an opaque per-endpoint
/// suffix, the Region and this service's endpoint prefix — behind the `vpc-` prefix
/// the service gives an endpoint that proxies into a VPC rather than the `search-`
/// one a domain's own endpoint carries, so the two are never mistaken for each other.
pub fn vpc_endpoint_host(domain_name: &str, suffix: &str, region: &str) -> String {
    format!("vpc-{domain_name}-{suffix}.{region}.{ARN_NAMESPACE}.amazonaws.com")
}

/// The endpoint a request REACHED this emulator at when that endpoint is the EMULATOR'S
/// OWN — a loopback address, an IP literal or any other host that names no service — and
/// `None` when the host names a service, which is a client that resolves AWS's own hosts.
///
/// Both readings it composes are `services/common`'s, so no second rule for "which host is
/// a service's" or "which host did this request address" is written here:
/// `data_plane_addressing::addressed_host` (the `Host` header, else an absolute URI's
/// authority) and `data_plane_addressing::names_a_service`, which defers to the front
/// door's own host reader.
///
/// What it decides is the ADDRESS a domain publishes for its data plane
/// ([`announced_domain_endpoint`]): a caller that reached this emulator at its own endpoint
/// resolves nothing else, so an AWS-shaped host told to it names no address it can reach.
pub fn local_endpoint_host(parts: &::http::request::Parts) -> Option<String> {
    let host = ::service_common::data_plane_addressing::addressed_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )?;
    (!::service_common::data_plane_addressing::names_a_service(&host)).then_some(host)
}

/// The address a domain of this service PUBLISHES for index, search and data-upload
/// requests, as announced to a caller that reached this emulator at `local_endpoint`: that
/// same endpoint, with the reserved path this service's data plane serves the domain at
/// (`<endpoint>/_aws/es/<domain>`, the addressing `crate::service::domain_rest` resolves).
///
/// `aws_host` is the host AWS publishes for that domain ([`domain_endpoint`] and the
/// VPC-access hosts beside it), which is what a caller that resolves AWS's own hosts is
/// told — so one reading decides every endpoint member of a domain's status, and a member
/// that names an address the caller cannot reach is never published.
pub fn announced_domain_endpoint(
    aws_host: &str,
    domain_name: &str,
    local_endpoint: Option<&str>,
) -> String {
    match local_endpoint {
        Some(endpoint) => format!(
            "{endpoint}/{}/{ARN_NAMESPACE}/{domain_name}",
            ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
        ),
        None => aws_host.to_string(),
    }
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

/// A `SystemTime` as the TEXT a member modelled a string carries an instant as:
/// date-time, which is how this service's string-typed timestamps
/// (`DryRunProgressStatus$CreationDate`, `$UpdateDate`) are written.
pub fn datetime_text(at: SystemTime) -> String {
    datetime(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// What this service says of a name that names no stored domain, in whichever
/// envelope the operation refusing it answers with: the ONE wording, so two
/// operations cannot word the same miss differently.
pub fn domain_not_found_message(domain_name: &str) -> String {
    format!("Domain not found: {domain_name}")
}

/// The `ResourceNotFoundException` payload for a name that names no stored domain.
pub fn domain_not_found(domain_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(domain_not_found_message(domain_name)),
    }
}

/// The `BaseException` payload for a name that names no stored domain — the generic
/// envelope (HTTP 400), for an operation that refuses an unknown domain with it
/// rather than with the not-found one.
pub fn domain_not_found_base(domain_name: &str) -> BaseException {
    BaseException {
        message: Some(domain_not_found_message(domain_name)),
    }
}

/// The `ResourceNotFoundException` payload for an id that addresses no cross-cluster
/// connection the caller has a view of — one this service never minted, one since
/// released, or one the caller's side of the connection has nothing to see yet.
///
/// The id the request carried is what fills the message, so a caller is told which of
/// several connections it named could not be resolved.
pub fn connection_not_found(connection_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Connection ID {connection_id} does not exist")),
    }
}

/// The `DisabledOperationException` payload for a decision on a connection that is
/// not waiting for one.
///
/// Accepting or rejecting an inbound connection request is available only while the
/// request waits for the destination domain owner's decision; past that the
/// connection is somewhere its lifecycle does not come back from, so the call is not
/// an operation that connection offers. The refusal names the status the connection
/// is actually in, so a caller can tell a decision already taken from one the
/// connection never waited for.
pub fn connection_decision_unavailable(
    connection_id: &str,
    status: &str,
) -> DisabledOperationException {
    DisabledOperationException {
        message: Some(format!(
            "Connection {connection_id} is in {status} state, not {CONNECTION_STATUS_PENDING_ACCEPTANCE}"
        )),
    }
}

/// The `ValidationException` payload for a change asked of a cross-cluster connection
/// whose current status does not allow it.
///
/// A connection the destination owner refused is that owner's record OF the refusal,
/// and a record of a decision is not something a later call takes back: the refusal
/// names the status the connection is in, because that status is the whole reason the
/// change cannot be made.
pub fn connection_change_unavailable(status: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "existing status is {status}, unable to make necessary change"
        )),
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

/// The `ResourceNotFoundException` payload for a name that names no index of a
/// domain the store DOES hold.
///
/// An index is a resource of its own — `CreateIndex` models
/// `ResourceAlreadyExistsException` for one the domain already has — so a call
/// naming one that does not exist is the modelled missing-resource refusal. The
/// wording the service uses for a missing index is fixed and names the `IndexName`
/// from the request only, so every index operation reports it identically.
pub fn index_not_found(index_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Resource index {index_name} does not exist")),
    }
}

/// The `ResourceAlreadyExistsException` payload for a create naming an index the
/// domain already holds, naming both the domain and the index so a caller can place
/// the collision.
pub fn index_exists(domain_name: &str, index_name: &str) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Index {index_name} already exists in domain {domain_name}."
        )),
    }
}

/// The `ValidationException` payload for a member that must be an `IndexName` and
/// is not ([`is_index_name`]). It names the member and the value, so the caller can
/// fix the request from the refusal alone. It is NOT a constraint report: the rule it
/// states is the search engine's own naming rule for an index, which no `@pattern` of
/// this model declares, so there is no expression to quote.
pub fn malformed_index_name(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must not be '.' or '..', must not begin with '_' or '-', and must carry \
             no upper-case letter, whitespace or any of ,:\"+/*\\|?#>< : {value}"
        )),
    }
}

/// The `ValidationException` payload for an `IndexSchema` that is not a JSON
/// object.
///
/// `IndexSchema` is a document "defining index mappings, settings, and semantic
/// enrichment configuration" — a set of named sections — so a document that carries
/// a scalar or an array instead is a malformed request rather than a schema with no
/// sections.
pub fn malformed_index_schema(member: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must be a JSON object of index schema sections."
        )),
    }
}

/// The `ValidationException` payload for an update that would re-type a field the
/// index already maps ([`mapping_type_conflict`]).
///
/// It names the field and both types, the way the search engine whose mapping the
/// update would contradict reports a mapping it cannot merge, so a caller can tell
/// which field of a many-field update was refused and why.
pub fn mapping_type_immutable(conflict: &MappingTypeConflict) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "mapper [{}] cannot be changed from type [{}] to [{}]",
            conflict.field, conflict.from, conflict.to
        )),
    }
}

/// The widths of a `GUID`'s five hexadecimal groups.
const GUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// `GUID`'s own `@pattern`, as the model declares it
/// (`smithy_query(query="shape",shape="GUID")`): the five hexadecimal groups
/// [`GUID_GROUP_WIDTHS`] states, as the expression a refusal of a malformed one quotes.
const GUID_PATTERN: &str =
    r"^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$";

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

/// The `ResourceNotFoundException` payload for a `DryRunId` a domain's history holds
/// no dry run for, and for a domain on which no dry run was ever started.
///
/// Worded as this service words every other miss of a record held ON a domain (a
/// configuration change, a maintenance action): the domain the request named and,
/// where it named one, the id it asked for.
pub fn dry_run_not_found(domain_name: &str, dry_run_id: Option<&str>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(match dry_run_id {
            Some(dry_run_id) => {
                format!("Dry run not found for domain {domain_name}: {dry_run_id}")
            }
            None => format!("No dry run found for domain {domain_name}"),
        }),
    }
}

/// The `ValidationException` payload for a member that must be a `GUID` and is not.
///
/// Stated as the constraint report every other value outside its member's declared
/// expression is refused with ([`crate::service::validation::constraint_report`]): the
/// request boundary enforces `GUID`'s `@length` but not its `@pattern`, so this is the
/// SAME constraint reported one step later, and a caller cannot tell the two apart.
/// `member` is the member's name as the model spells it.
pub fn malformed_guid(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(crate::service::validation::constraint_report(
            member,
            value,
            ::aws_smithy_schema_server::ViolationKind::Pattern(GUID_PATTERN.to_owned()),
        )),
    }
}

/// The `ValidationException` payload for an integer member below the minimum its
/// modelled `@range` sets, in the envelope AWS states a constraint violation in: the
/// value the request carried, the member it was carried at as the wire names it, and
/// the bound it broke.
///
/// Stated through [`crate::service::validation::constraint_report`], the one builder of
/// that envelope, so a bound a handler checks reads like one the boundary checked.
pub fn below_range_minimum(member: &str, value: i32, minimum: i32) -> ValidationException {
    ValidationException {
        message: Some(crate::service::validation::constraint_report(
            member,
            &value.to_string(),
            ::aws_smithy_schema_server::ViolationKind::MinValue(minimum.to_string()),
        )),
    }
}

/// The `ResourceNotFoundException` payload for a domain that has no pending
/// configuration change for a cancel to cancel.
pub fn no_pending_change(domain_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "No pending configuration change to cancel for domain {domain_name}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for a `NodeId` that names no data node of
/// the domain a maintenance action was asked for on, or one whose role cannot lose a
/// node ([`MAINTAINABLE_DATA_NODES`]) — the node is then not one this service can
/// maintain, which is what makes the id invalid.
pub fn invalid_node_id() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("The requested node ID is invalid".to_string()),
    }
}

/// The HTTP status this service answers a maintenance LISTING's not-found refusal
/// with.
///
/// `ResourceNotFoundException` carries `@httpError(409)` in the model, which is the
/// status the shape renders by default and the one `StartDomainMaintenance` and
/// `GetDomainMaintenanceStatus` are recorded answering. The wire answer recorded for
/// `ListDomainMaintenances` is HTTP 404 instead, so that operation states the status
/// rather than taking the shape's — see
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`], which is the only place a
/// status may differ from the model's and requires exactly this kind of evidence.
pub const MAINTENANCE_LISTING_NOT_FOUND_STATUS: u16 = 404;

/// The `ValidationException` payload for a maintenance request that names no node:
/// the action is "the node maintenance process on the data node", so there is no
/// action to start without one.
pub fn node_id_required() -> ValidationException {
    ValidationException {
        message: Some("NodeId is required to start domain maintenance".to_string()),
    }
}

/// The `ResourceNotFoundException` payload for a `MaintenanceId` a domain's history
/// holds no maintenance action for.
pub fn maintenance_not_found(domain_name: &str, maintenance_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Maintenance not found for domain {domain_name}: {maintenance_id}"
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

/// `entity`'s `Value`, or `None` where the request named none — an absent value and
/// a blank one name the same nothing.
fn insight_entity_value(entity: &InsightEntity) -> Option<&str> {
    entity
        .value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// The `ValidationException` payload for an insight entity that names a domain but
/// no domain name.
pub fn insight_value_required() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity value is required when Entity type is {INSIGHT_ENTITY_DOMAIN_NAME}."
        )),
    }
}

/// The `ValidationException` payload for an `Account` insight entity naming an
/// account other than the one the request was made for.
pub fn foreign_account_entity(account_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity value {account_id} is not the Amazon Web Services account ID this request \
             was made for."
        )),
    }
}

/// The `ValidationException` payload for an insight entity whose type is outside
/// `InsightEntityType`.
pub fn unknown_insight_entity(entity_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity type {entity_type} is not supported. Supported values are \
             {INSIGHT_ENTITY_ACCOUNT} and {INSIGHT_ENTITY_DOMAIN_NAME}."
        )),
    }
}

/// The `ValidationException` payload for a feedback entity whose type is outside
/// `InsightFeedbackEntityType` — which admits `DomainName` alone, so feedback accepts
/// strictly fewer entity types than a listing does.
pub fn unknown_insight_feedback_entity(entity_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Entity type {entity_type} is not supported. Supported values are \
             {INSIGHT_ENTITY_DOMAIN_NAME}."
        )),
    }
}

/// The `ValidationException` payload for a feedback rating outside
/// `InsightFeedbackThumbs`.
pub fn unknown_insight_feedback_thumbs(thumbs: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Thumbs {thumbs} is not supported. Supported values are {} and {}.",
            INSIGHT_FEEDBACK_THUMBS_UP, INSIGHT_FEEDBACK_THUMBS_DOWN
        )),
    }
}

/// The `ValidationException` payload for feedback text past the longest the modelled
/// member admits: it names the bound and the length the submission carried, so the
/// caller can fix the request from the refusal alone.
pub fn feedback_text_too_long(length: usize, maximum: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "FeedbackText must be at most {maximum} characters: {length}"
        )),
    }
}

/// What a feedback submission says of a resource it cannot resolve: the submission
/// named an insight of a domain, and neither the domain nor the insight resolved to
/// something this account holds.
///
/// The wording is the generic one this operation refuses with, naming no resource:
/// one refusal for both halves of the address, so a caller cannot tell a domain it
/// does not hold from an insight that domain does not hold — which is what the
/// service answers.
pub fn insight_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Resource not found".to_string()),
    }
}

/// The entity a request named, as a detail field names it: the domain an entity of
/// that type names, or the account an `Account` entity stands for — the one this
/// request was made for where it named no id.
fn insight_entity_label(entity: &InsightEntity) -> String {
    if entity.r#type == INSIGHT_ENTITY_DOMAIN_NAME {
        format!("domain {}", insight_entity_value(entity).unwrap_or_default())
    } else {
        format!(
            "account {}",
            insight_entity_value(entity).unwrap_or(::engine::emulator_account_id())
        )
    }
}

/// Where the insights of the entity a request named are reviewed: this service's
/// console home in `region`, at the entity's own place in it.
///
/// CHOSEN, not documented by AWS: no AWS document gives the console location of an
/// entity's insights a form, so this emulator serves one deterministic form of its
/// own, built from the entity the request named and the Region it was signed for.
fn insights_console_uri(entity: &InsightEntity, region: &str) -> String {
    let home = format!("https://{region}.console.aws.amazon.com/aos/home?region={region}");
    match insight_entity_value(entity) {
        Some(name) if entity.r#type == INSIGHT_ENTITY_DOMAIN_NAME => {
            format!("{home}#opensearch/domains/{name}")
        }
        _ => format!("{home}#opensearch/insights"),
    }
}

/// The detail fields a read of an insight id the entity holds no insight under is
/// answered with: informational fields, naming the id that resolved to no insight of
/// that entity and where the entity's insights are reviewed.
///
/// A detail read is not keyed on an insight existing under the requested id — any id
/// of the modelled `GUID` form is answered rather than refused — and `Fields` is
/// `@required`, so the view carries the service's informational fields where it holds
/// no insight to detail. Each field is built from the request's id and entity and the
/// Region it was signed for, so the same request is always answered the same way and
/// no stored insight is involved.
pub fn informational_insight_details(
    entity: &InsightEntity,
    insight_id: &str,
    region: &str,
) -> Vec<InsightDetailField> {
    let held_by = insight_entity_label(entity);
    let console = insights_console_uri(entity, region);
    vec![
        InsightDetailField {
            name: "Description".to_string(),
            field_type: INSIGHT_FIELD_TYPE_TEXT.to_string(),
            value: format!(
                "No insight details are available for insight {insight_id} of {held_by}. \
                 Review the insights of {held_by} in the Amazon OpenSearch Service console."
            ),
            html_value: Some(format!(
                "<p>No insight details are available for insight {insight_id} of {held_by}. \
                 Review the insights of {held_by} in the Amazon OpenSearch Service \
                 console.</p>"
            )),
        },
        InsightDetailField {
            name: "Console".to_string(),
            field_type: INSIGHT_FIELD_TYPE_TEXT.to_string(),
            value: console,
            // The console location is reported as the URI itself in either rendering:
            // it is a value a caller follows, not content the service marks up.
            html_value: None,
        },
    ]
}

/// The `ValidationException` payload for an upgrade whose `TargetVersion` is not a
/// version the domain can be upgraded to: it names the version the domain runs, the
/// one that was asked for and the ones it could have been
/// ([`compatible_target_versions`]), so the caller can fix the request from the
/// refusal alone.
pub fn incompatible_target_version(
    domain_name: &str,
    source_version: &str,
    target_version: &str,
) -> ValidationException {
    let targets = compatible_target_versions(source_version);
    ValidationException {
        message: Some(if targets.is_empty() {
            format!(
                "Domain {domain_name} runs {source_version} and cannot be upgraded to \
                 {target_version}: there is no version to upgrade it to."
            )
        } else {
            format!(
                "Domain {domain_name} runs {source_version} and cannot be upgraded to \
                 {target_version}. Compatible target versions: {}.",
                targets.join(", ")
            )
        }),
    }
}

/// The `ResourceAlreadyExistsException` payload for an upgrade asked for while the
/// domain is still performing one: that run already exists, so the refusal names it
/// rather than the call starting a second.
pub fn upgrade_already_in_progress(
    domain_name: &str,
    upgrade_id: &str,
) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Domain {domain_name} is already performing upgrade {upgrade_id}."
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

/// The `ValidationException` payload for a page size outside the range the listing's
/// `MaxResults` publishes.
///
/// `MaxResults` is `@range(max: 100)` with `@default(0)`: a page size past the maximum
/// is a malformed request, and a NEGATIVE one is malformed for the same reason — it is
/// not a page size at all — while the default zero is the request that asks for no
/// particular page size. `limit` is the maximum the ASKING operation publishes, because
/// a listing may bound its page below what the shape allows.
pub fn max_results_out_of_range(max_results: i32, limit: i32) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "MaxResults must be between 0 and {limit}: {max_results}"
        )),
    }
}

/// The `ValidationException` payload for a NEGATIVE page size, for a listing whose
/// `maxResults` publishes no maximum at all.
///
/// A negative value is not a page size, so it is refused rather than reinterpreted,
/// while the default zero stays the request that asks for no particular page size.
/// There is no upper bound to report because the shape declares none: a page size past
/// the size of the set is simply the whole set.
pub fn negative_max_results(max_results: i32) -> ValidationException {
    ValidationException {
        message: Some(format!("MaxResults must not be negative: {max_results}")),
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
///
/// The text is FIXED and carries no variable part — in particular it does not quote
/// the token back, which is the caller's own input and tells it nothing it did not
/// send — so every operation that refuses a token with this error words the refusal
/// identically.
pub fn invalid_pagination_token() -> InvalidPaginationTokenException {
    InvalidPaginationTokenException {
        message: Some("Pagination token provided is invalid".to_string()),
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

/// The minimum a purchase may reserve, from
/// `PurchaseReservedInstanceOffering$InstanceCount`'s `@range(min: 1)`. The member
/// carries no `@default`, so a request that names no count names a count below this
/// bound rather than one the service fills in.
pub const INSTANCE_COUNT_MINIMUM: i32 = 1;

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
/// A well-formed identifier that addresses no record names no resource, so this is
/// the modelled `ResourceNotFoundException` (HTTP 409) — the same refusal the
/// offering catalog answers an id it publishes nothing for with
/// ([`offering_not_found`]) — in the service's own wording for a reservation id that
/// resolves to nothing.
pub fn reserved_instance_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Reservation id not found".to_string()),
    }
}
