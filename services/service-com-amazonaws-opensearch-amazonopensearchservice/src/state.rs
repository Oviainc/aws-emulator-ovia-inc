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
//!
//! This module is the ROOT of the store: the catalogs and timings every part of it
//! reads, the records of a domain's own lifecycle (its configuration changes, its
//! service software, its upgrades and its scheduled actions), and one child module per
//! other concern — each declared below with what it holds, and re-exported here, so a
//! handler reaches every record through `crate::state` whichever file it lives in.

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

/// The [`Domain`] record every domain operation reads and writes, the projections the
/// model asks for it, and where its nodes sit.
mod domains;
/// The store every operation reaches this service's records through, and the ids it mints.
mod store;

/// A principal's access to one domain through an interface VPC endpoint, and the
/// service-managed endpoints themselves.
mod vpc_endpoints;
/// The network a VPC endpoint is placed in: which VPC and Availability Zone each of its
/// subnets is in, resolved against the service that owns subnets.
pub mod vpc_network;

/// An index of a domain, the schema it carries and the settings around it.
mod indexes;
/// A package of this account and its association with a domain.
mod packages;

/// Which instance types an engine version offers, what each can be scaled to, and the
/// domain configurations that catalog refuses.
mod instance_types;
/// The Reserved Instance offerings this service publishes.
mod reserved_instance_offerings;
/// A Reserved Instance an account bought from one of those offerings.
mod reserved_instances;

/// The INPUT shapes a request carries, as the OUTPUT shapes a record stores and a
/// descriptor reports, and the endpoint names a domain publishes.
mod projections;
/// The payloads this service refuses a request with, one per condition.
mod refusals;

pub use domains::*;
pub use indexes::*;
pub use instance_types::*;
pub use packages::*;
pub use projections::*;
pub use refusals::*;
pub use reserved_instance_offerings::*;
pub use reserved_instances::*;
pub use store::*;
pub use vpc_endpoints::*;

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
        message: Some(format!("Application name {name} already exists")),
    }
}

/// What the application CRUD surface states about an id that names no application of
/// this account.
///
/// The statement names no id: `GetApplication`, `UpdateApplication` and
/// `DeleteApplication` address ONE application, so the id the refusal could not resolve
/// is the one the request is about and the answer states only that the account holds no
/// such application.
pub const APPLICATION_DOES_NOT_EXIST: &str = "Application does not exist";

/// What the sub-APIs of an application state about the same miss: the id the request
/// carried.
///
/// A call on an application's DATA SOURCES or its MIGRATION jobs is about one of those
/// sub-resources, so the application is a value the request named among others and the
/// refusal names it back — which is what tells a caller handed several ids which one
/// could not be resolved.
pub fn application_not_found_message(id: &str) -> String {
    format!("Application {id} not found")
}

/// The same statement with the account the lookup was scoped to named after it, for the
/// path whose refusal states both — so an operator can see the account the id was
/// resolved in and not only that it was not found.
pub fn application_not_found_for_account_message(id: &str, account_id: &str) -> String {
    format!(
        "{} for account {account_id}",
        application_not_found_message(id)
    )
}

/// The `ResourceNotFoundException` payload the application CRUD surface answers an id
/// that names no application of this account with ([`APPLICATION_DOES_NOT_EXIST`]).
pub fn application_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(APPLICATION_DOES_NOT_EXIST.to_string()),
    }
}

/// The `ResourceNotFoundException` payload a SUB-API of an application answers the same
/// miss with, naming the id ([`application_not_found_message`]).
pub fn application_not_found_named(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(application_not_found_message(id)),
    }
}

/// The same refusal, naming the account the id was resolved in
/// ([`application_not_found_for_account_message`]).
pub fn application_not_found_for_account(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(application_not_found_for_account_message(
            id,
            ::engine::emulator_account_id(),
        )),
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

/// The `ResourceNotFoundException` payload for an application and data source pair the
/// SINGLE-ATTACHMENT READ holds no attachment record for: "throws a
/// ResourceNotFoundException if no attachment record exists for the specified
/// application and data source combination".
///
/// The DETACH states the same miss as [`data_source_not_attached`] — the recorded
/// sentences of the two operations differ, so each states its own and neither is a
/// rewording of the other.
pub fn attachment_not_found(id: &str, data_source_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Data source attachment not found for application {id} and data source \
             {data_source_arn}"
        )),
    }
}

/// The `ResourceNotFoundException` payload the DETACH states for a data source the
/// application holds no attachment record for: what the caller asked to undo was never
/// done, so the sentence is about the link rather than about a record.
pub fn data_source_not_attached(id: &str, data_source_arn: &str) -> ResourceNotFoundException {
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
            "Data source {data_source_arn} attachment to application {id} is still \
             {ATTACHMENT_STATUS_PENDING}."
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
/// an application this account DOES hold — the migration mirror of
/// [`application_not_found`].
pub fn migration_not_found(migration_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Migration not found: {migration_id}")),
    }
}

/// The application a `migrationId` belongs to, as the id itself names it: the text
/// before its first hyphen.
///
/// A migration is addressed by its id ALONE — `GetMigration` takes no `applicationId`
/// (`smithy_query(query="operation",operation="GetMigration")`) — so the id has to carry
/// the application whose workspace the job imports into, and the id this service mints
/// ([`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_migration_id`]) does. An
/// `Id` is `^[a-z0-9]{3,30}$`, which carries no hyphen, so the first hyphen is where the
/// application's part of the id ends.
///
/// An id carrying NO hyphen names no application and no job either, so the whole of it is
/// what the refusal has to state; whether a value that is no id of this service at all is
/// better refused for its SHAPE, before it is resolved, is the operation's own validation
/// question rather than this resolution's.
pub fn migration_application_id(migration_id: &str) -> &str {
    migration_id
        .split_once('-')
        .map_or(migration_id, |(application_id, _)| application_id)
}

/// The expression this service validates a `migrationId` against, as a whole-value
/// pattern: the `ApplicationId` the id names — `^[a-z0-9]{3,30}$`
/// (`smithy_query(query="shape",shape="ApplicationId")`) — then the hyphen and the
/// opaque part a start minted after it
/// ([`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_migration_id`]).
///
/// The model types `migrationId` a plain `String` and declares no pattern for it, so
/// this is the surface the SERVICE validates. BOTH halves are required: a read is
/// addressed by the id ALONE, so an id that does not carry an application and a job
/// addresses nothing in any account, whichever applications one holds. What the opaque
/// half SPELLS is not constrained — it is the service's to mint and the caller's to
/// quote back.
///
/// DOCUMENTATION of the rule [`is_migration_id`] applies, not the check itself: the
/// expression is never compiled or matched against, because no `@pattern` of this model
/// declares it and nothing in this crate runs a regex engine. The end-to-end table over
/// that rule (`tests/all/control_plane_refusal_wording.rs`, one row per way a value
/// leaves the form) is what keeps this sentence and that check from drifting.
pub const MIGRATION_ID_PATTERN: &str = "^[a-z0-9]{3,30}-.+$";

/// Whether `value` is a `migrationId` of the form [`MIGRATION_ID_PATTERN`] states.
///
/// A SHAPE test only, the way [`is_guid`] and [`is_index_name`] are: whether the id
/// resolves to an application this account holds, and to a job of it, is the lookup's
/// job. It is tested here rather than against the pattern text because the boundary's
/// regex engine never sees a `@pattern` the model does not declare.
pub fn is_migration_id(value: &str) -> bool {
    let Some((application_id, opaque)) = value.split_once('-') else {
        return false;
    };
    (3..=30).contains(&application_id.len())
        && application_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !opaque.is_empty()
}

/// How this service states that a `migrationId` is not one: AWS's own sentence for the
/// condition, naming the member and the value the request carried.
///
/// The value is quoted through the ceiling every clause of this service echoes a
/// submitted value under ([`::service_common::validation::quoted_echo`]): `migrationId`
/// declares no `@length` of its own, so without it one request would size the response.
pub fn invalid_migration_id_format(value: &str) -> String {
    format!(
        "Invalid migrationId format: {}",
        ::service_common::validation::quoted_echo(value)
    )
}

/// The `ValidationException` payload for a `migrationId` outside
/// [`MIGRATION_ID_PATTERN`]: a value that names neither an application nor a job carries
/// no reference to a resource, so it is a malformed request rather than a job that
/// happens not to exist.
pub fn malformed_migration_id(value: &str) -> ValidationException {
    ValidationException {
        message: Some(invalid_migration_id_format(value)),
    }
}

/// The `ResourceNotFoundException` payload for a `migrationId` whose application
/// ([`migration_application_id`]) is one this account does not hold: there is no
/// application to resolve the job in, so what the refusal states is the missing
/// application rather than the job.
///
/// It is [`application_not_found_named`] itself: one sentence for "this account holds no
/// such application" across the sub-APIs of an application, naming the id the request
/// carried — here the id read out of the `migrationId`, which is the only place the
/// caller can see which application the id resolved to.
pub fn migration_application_not_found(application_id: &str) -> ResourceNotFoundException {
    application_not_found_named(application_id)
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

/// The same missing-application statement as [`application_not_found_named`], carried by
/// the client-input error an operation that models NO `ResourceNotFoundException` does
/// model.
///
/// One wording for "this account holds no such application" across the sub-APIs,
/// whichever error channel the asking operation has for it.
pub fn unknown_application_id(id: &str) -> ValidationException {
    ValidationException {
        message: Some(application_not_found_message(id)),
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

// ---------------------------------------------------------------------------
// The Reserved Instance offering catalog
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Input -> stored projections
//
// Several option containers have an INPUT shape a request carries and an OUTPUT
// shape the service reports. The record stores the output shape, so the
// conversion happens once, here, rather than once per operation that accepts the
// option.
// ---------------------------------------------------------------------------

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
