// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-directoryservice-directoryservice-20150416 service.
//!
//! This module owns the records the handlers in `src/ops/` share — the directories
//! `CreateDirectory` persists, the snapshots `CreateSnapshot` takes of them, the aliases
//! `CreateAlias` assigns, the computer accounts `CreateComputer` creates in them, the
//! Amazon SNS topics `RegisterEventTopic` associates, the trust relationships
//! `CreateTrust` establishes, the assessments
//! `StartADAssessment` starts and the tags `AddTagsToResource` merges — plus
//! the pieces of behaviour that cannot live in any single operation file:
//!
//! * [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`], which
//!   materialises the status transitions the clock has already made due (a directory
//!   leaving `Requested`, a snapshot leaving `Creating`, an IP route leaving `Adding`, a
//!   trust leaving `Creating` or `Verifying` (and a deleted one leaving the store), an
//!   assessment leaving `IN_PROGRESS`, a directory's data access leaving `Enabling`).
//!   Every handler calls it before reading or writing, so a reporting operation reports
//!   what the clock already owed and nothing more.
//! * [`Directory::to_description`], the one place a stored directory is rendered as the
//!   modelled `DirectoryDescription`, so every operation that reports a directory
//!   reports the same derived values (`Alias`, `AccessUrl`), and
//!   [`SnapshotRecord::to_snapshot`] / [`EventTopicRegistration::to_event_topic`] /
//!   [`TrustRecord::to_trust`], the
//!   same for a stored snapshot, a stored topic association and a stored trust.
//! * [`encode_cursor`] / [`decode_cursor`], the opaque `NextToken` the paginated
//!   operations of this slice (`DescribeDirectories`, `DescribeSnapshots`,
//!   `DescribeTrusts`, `ListTagsForResource`) issue and resolve.
//! * [`refusal`], the one place a modelled refusal of this service is built: every error
//!   shape the model declares carries a `Message` and a UUID-shaped `RequestId`, and this
//!   is what mints the call's id, reports it in that member and appends it to the message
//!   — so no handler can answer a refusal without the correlation handle a client logs it
//!   by. [`invalid_resource_id`] and [`invalid_next_token`] are the two diagnostics a
//!   whole family of operations shares, worded once here beside [`no_such_directory`].
//!
//! The administrator `Password` `CreateDirectory` is called with is deliberately NOT a
//! field here: the model marks it `@sensitive`, no operation reports it back, and state
//! this emulator does not need is state it must not hold.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Assessment, AssessmentSummary, Attribute, ClientAuthenticationSettingInfo, Computer,
    ConditionalForwarder, DirectoryDescription, DirectoryLimits, DirectoryVpcSettingsDescription,
    EventTopic, IpRouteInfo, LDAPSSettingInfo, Snapshot, SnapshotLimits, Tag, Trust,
};

/// How long a status transition a create owes takes to fall due.
///
/// A create (`CreateDirectory`, `CreateMicrosoftAD`) answers while the directory is
/// still being built — the modelled
/// `DirectoryStage` enum's first value is `Requested` — `CreateSnapshot` answers
/// while the snapshot is still being taken — the modelled `SnapshotStatus` enum's first
/// value is `Creating` — and `StartADAssessment` answers while the run it started is
/// still `IN_PROGRESS`, so the move to `Active`/`Completed`/a terminal assessment status
/// is owed by the clock rather
/// than by any later call. The project's emulator contract requires such a transition to
/// fall due 250–750 ms after the call that starts it, so a caller acting immediately
/// after the create still observes the transient state. 500 ms sits in the middle of
/// that band.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The stage a directory is in between `CreateDirectory` and [`TRANSITION_SPAN`].
pub const STAGE_REQUESTED: &str = "Requested";

/// The `IpRouteStatusMsg` an address block carries between `AddIpRoutes` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `IpRouteStatusMsg` enum.
///
/// `AddIpRoutes` answers before the block is in service, so the move to
/// [`IP_ROUTE_STATUS_ADDED`] is owed by the clock, on the same span a create's stage
/// transition uses.
pub const IP_ROUTE_STATUS_ADDING: &str = "Adding";

/// The `IpRouteStatusMsg` an address block reaches once [`TRANSITION_SPAN`] has
/// elapsed: the `Added` value of the modelled `IpRouteStatusMsg` enum.
pub const IP_ROUTE_STATUS_ADDED: &str = "Added";

/// The status a snapshot is in between `CreateSnapshot` and [`TRANSITION_SPAN`]: the
/// first value of the modelled `SnapshotStatus` enum.
pub const SNAPSHOT_STATUS_CREATING: &str = "Creating";

/// The status a snapshot reaches once [`TRANSITION_SPAN`] has elapsed: the `Completed`
/// value of the modelled `SnapshotStatus` enum.
pub const SNAPSHOT_STATUS_COMPLETED: &str = "Completed";

/// The `SnapshotType` of a snapshot a caller asked for: the `Manual` value of the
/// modelled `SnapshotType` enum. The enum's other value, `Auto`, is for the snapshots
/// Directory Service takes on its own schedule, which no caller of this emulator starts.
pub const SNAPSHOT_TYPE_MANUAL: &str = "Manual";

/// The `DirectoryType` values a snapshot can be taken of.
///
/// From `CreateSnapshot`'s own documentation: "Creates a snapshot of a Simple AD or
/// Microsoft AD directory in the Amazon Web Services cloud. You cannot take snapshots of
/// AD Connector directories." The modelled `DirectoryType` enum's remaining value,
/// `ADConnector`, is therefore refused, and `SharedMicrosoftAD` — a directory shared from
/// another account, which the owner snapshots — with it.
pub const SNAPSHOTTABLE_DIRECTORY_TYPES: [&str; 2] = [TYPE_SIMPLE_AD, "MicrosoftAD"];

/// The stage a created directory reaches once [`TRANSITION_SPAN`] has elapsed.
pub const STAGE_ACTIVE: &str = "Active";

/// The `DirectoryType` of a directory created by `CreateDirectory`: Simple AD.
///
/// `CreateDirectory`'s own documentation ("Creates a Simple AD directory"), rendered
/// with the `SimpleAD` value of the modelled `DirectoryType` enum.
pub const TYPE_SIMPLE_AD: &str = "SimpleAD";

/// The `DirectoryType` of a directory created by `CreateMicrosoftAD`: AWS Managed
/// Microsoft AD.
///
/// `CreateMicrosoftAD`'s own documentation ("Creates a Microsoft AD directory ... see
/// Managed Microsoft AD"), rendered with the `MicrosoftAD` value of the modelled
/// `DirectoryType` enum.
pub const TYPE_MICROSOFT_AD: &str = "MicrosoftAD";

/// The editions a Managed Microsoft AD directory may be created with.
///
/// From `CreateMicrosoftADRequest$Edition`'s own documentation: "Managed Microsoft AD is
/// available in two editions: `Standard` and `Enterprise`." The third value of the
/// modelled `DirectoryEdition` enum, `Hybrid`, belongs to a hybrid directory and is
/// therefore refused by `CreateMicrosoftAD`.
pub const MICROSOFT_AD_EDITIONS: [&str; 2] = ["Standard", "Enterprise"];

/// The edition a Managed Microsoft AD directory is created with when the request names
/// none, from the same sentence: "`Enterprise` is the default."
pub const DEFAULT_MICROSOFT_AD_EDITION: &str = "Enterprise";

/// The number of domain controllers a Managed Microsoft AD directory is created with.
///
/// The modelled `DesiredNumberOfDomainControllers` carries `range { min: 2 }`, and
/// `DirectoryVpcSettings$SubnetIds`' own documentation says "Directory Service creates a
/// directory server and a DNS server in each of these subnets" — two subnets in
/// different Availability Zones — so a freshly created Managed Microsoft AD directory
/// has exactly the modelled minimum of two. `UpdateNumberOfDomainControllers` is what
/// changes it afterwards.
pub const MICROSOFT_AD_DOMAIN_CONTROLLERS: i32 = 2;

/// The `DirectoryType` of an AD Connector: the `ADConnector` value of the modelled
/// `DirectoryType` enum, which `ConnectDirectory` (still scaffolded) is what persists.
pub const TYPE_AD_CONNECTOR: &str = "ADConnector";

/// The DNS suffix a directory's access URL is built on, from the `AccessUrl` member's
/// documentation: "The access URL for the directory, such as
/// `http://<alias>.awsapps.com`".
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// The network types a Simple AD directory may be created with.
///
/// From `CreateDirectoryRequest$NetworkType`'s own documentation: "The network type for
/// your directory. Simple AD supports IPv4 and Dual-stack only." The third value of the
/// modelled `NetworkType` enum, `IPv6`, is therefore refused for this directory type.
pub const SIMPLE_AD_NETWORK_TYPES: [&str; 2] = ["IPv4", "Dual-stack"];

/// Directory Service's default quota of Simple AD directories per account per Region,
/// which `CreateDirectory` draws on.
///
/// From the Directory Service quotas the service publishes — "Simple AD directories:
/// 10" per Region — which is also the limit `GetDirectoryLimits` reports as
/// `CloudOnlyDirectoriesLimit` ("The maximum number of cloud directories allowed in the
/// Region").
pub const MAX_CLOUD_ONLY_DIRECTORIES: usize = 10;

/// Directory Service's default quota of AWS Managed Microsoft AD directories per account
/// per Region, which `CreateMicrosoftAD` draws on.
///
/// From the same published quotas — "AWS Managed Microsoft AD directories: 20" per
/// Region — which is the limit `GetDirectoryLimits` reports as
/// `CloudOnlyMicrosoftADLimit` ("The maximum number of Managed Microsoft AD directories
/// allowed in the region"). It is DOUBLE the Simple AD allowance: each category of
/// directory carries its own quota, so no counter of the modelled `DirectoryLimits` can
/// be reported from another's.
pub const MAX_CLOUD_ONLY_MICROSOFT_AD_DIRECTORIES: usize = 20;

/// Directory Service's default quota of AD Connector directories per account per Region,
/// which `ConnectDirectory` (still scaffolded) would draw on.
///
/// From the same published quotas — "AD Connector directories: 10" per Region — which is
/// the limit `GetDirectoryLimits` reports as `ConnectedDirectoriesLimit` ("The maximum
/// number of connected directories allowed in the Region").
pub const MAX_CONNECTED_DIRECTORIES: usize = 10;

/// The most directories one Region can hold at once, whatever their type: the three
/// published per-category quotas together.
///
/// NOT a quota of its own — every create counts against ITS OWN category's quota
/// ([`DirectoryLimitCategory::quota`]) — but the ceiling of the directory store, and so
/// the figure the resource guards derived from that store's size are sized against.
pub const MAX_DIRECTORIES: usize = MAX_CLOUD_ONLY_DIRECTORIES
    + MAX_CLOUD_ONLY_MICROSOFT_AD_DIRECTORIES
    + MAX_CONNECTED_DIRECTORIES;

/// Which of the three counters the modelled `DirectoryLimits` structure carries a
/// stored directory counts towards, decided by its `DirectoryType`.
///
/// `DirectoryLimits` reports a (limit, current count, limit reached) triple for
/// cloud-only directories, for cloud-only Managed Microsoft AD directories and for
/// connected directories, while the modelled `DirectoryType` enum carries four values.
/// The fourth, `SharedMicrosoftAD`, belongs to none of the three: it is a directory
/// another account shared into this one rather than one created here, and the structure
/// models no counter for it — hence [`Self::of`] answering `Option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryLimitCategory {
    /// A cloud-only directory — [`TYPE_SIMPLE_AD`].
    CloudOnly,
    /// A cloud-only AWS Managed Microsoft AD directory — [`TYPE_MICROSOFT_AD`].
    CloudOnlyMicrosoftAd,
    /// A directory connected to an on-premises one — [`TYPE_AD_CONNECTOR`].
    Connected,
}

impl DirectoryLimitCategory {
    /// The counter a stored `DirectoryType` counts towards, or `None` when
    /// `DirectoryLimits` models no counter for that type.
    pub fn of(directory_type: &str) -> Option<Self> {
        match directory_type {
            TYPE_SIMPLE_AD => Some(Self::CloudOnly),
            TYPE_MICROSOFT_AD => Some(Self::CloudOnlyMicrosoftAd),
            TYPE_AD_CONNECTOR => Some(Self::Connected),
            _ => None,
        }
    }

    /// This category's own published Region quota: the limit `GetDirectoryLimits`
    /// reports for its counter, and the bound a create of that category is refused past
    /// with `DirectoryLimitExceededException`.
    pub fn quota(self) -> usize {
        match self {
            Self::CloudOnly => MAX_CLOUD_ONLY_DIRECTORIES,
            Self::CloudOnlyMicrosoftAd => MAX_CLOUD_ONLY_MICROSOFT_AD_DIRECTORIES,
            Self::Connected => MAX_CONNECTED_DIRECTORIES,
        }
    }
}

/// AWS's tagging limit of 50 tags per resource, the bound
/// `TagLimitExceededException` reports.
pub const MAX_TAGS: usize = 50;

/// The account quota on directory assessments, the bound
/// `ADAssessmentLimitExceededException` reports.
///
/// From `StartADAssessment`'s own documentation: "Your Amazon Web Services account has a
/// limit of 100 CUSTOMER directory assessments."
pub const MAX_CUSTOMER_ASSESSMENTS: usize = 100;

/// The status an assessment carries while the run it started is still owed by the
/// clock, from `Assessment$Status`'s documentation: "Valid values include `SUCCESS`,
/// `FAILED`, `PENDING`, and `IN_PROGRESS`."
pub const ASSESSMENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status an assessment reaches in this emulator, from the same
/// `Assessment$Status` value list. See [`ASSESSMENT_FAILED_REASON`] for why it is this
/// value and not `SUCCESS`.
pub const ASSESSMENT_STATUS_FAILED: &str = "FAILED";

/// Why every assessment this emulator starts ends `FAILED`.
///
/// An assessment "checks compatibility and connectivity of the self-managed AD
/// environment" (`StartADAssessment`'s documentation) by probing the customer's DNS
/// servers and managed nodes. This emulator makes no outbound connection, so that run
/// cannot be performed: the assessment resource is real — it is minted, listed,
/// described and counted against the account quota — and the run it starts ends in the
/// modelled failure status carrying this reason, rather than reporting validations that
/// nothing performed. It is reported through `Assessment$StatusReason`, documented as
/// "A human-readable description of the current assessment status, including any error
/// details or progress information."
pub const ASSESSMENT_FAILED_REASON: &str = "The assessment could not be run: this emulator has no network path to the self-managed AD environment, so no connectivity or compatibility validation was performed.";

/// The `ReportType` of an assessment started by `StartADAssessment`.
///
/// From `Assessment$ReportType`'s documentation ("Valid values are `CUSTOMER` and
/// `SYSTEM`") and `StartADAssessment`'s ("A directory assessment is automatically
/// created when you create a hybrid directory. There are two types of assessments:
/// CUSTOMER and SYSTEM") — a caller-started assessment is the CUSTOMER kind, and it is
/// the CUSTOMER ones [`MAX_CUSTOMER_ASSESSMENTS`] bounds.
pub const ASSESSMENT_REPORT_TYPE_CUSTOMER: &str = "CUSTOMER";

/// The number of IP address blocks a directory may carry, from
/// `IpRouteLimitExceededException`'s own documentation: "The maximum allowed number of
/// IP addresses was exceeded. The default limit is 100 IP address blocks."
pub const MAX_IP_ROUTES: usize = 100;

/// The first RID this emulator hands to a computer account it creates in a directory.
///
/// A Windows account's SID is its domain's SID plus a relative identifier, and Active
/// Directory reserves the RIDs below 1000 for the built-in principals every domain is
/// created with, handing the ones from 1000 up to the accounts created in it. So the
/// first computer account of a directory is `<domain SID>-1000` and each later one takes
/// the next RID, which is what makes a `ComputerId` unique within its directory.
pub const FIRST_COMPUTER_RID: u32 = 1000;

/// The prefix every Active Directory domain account's SID carries: the NT authority
/// (`S-1-5`) and the `SECURITY_NT_NON_UNIQUE` sub-authority (`21`) that marks the three
/// sub-authorities after it as one domain's own identifier. See
/// [`Directory::domain_sid`], which is what appends those three.
pub const DOMAIN_SID_PREFIX: &str = "S-1-5-21";

/// The most computer accounts this emulator keeps in one directory.
///
/// NOT an AWS quota: a resource guard, like [`MAX_SNAPSHOTS`]. `CreateComputer` models no
/// limit-exceeded error and AWS publishes a directory's capacity as an approximate object
/// count rather than an API limit, so this is not an emulated quota — it is what keeps a
/// loop of creates from growing the store without end, set at the smallest capacity AWS
/// publishes for a directory (a Simple AD `Small` directory, documented as up to 500
/// users / roughly 2,000 directory objects). Past it `CreateComputer` is refused with the
/// generic `ClientException` the model offers, since the model declares no bound-specific
/// error for this operation.
pub const MAX_COMPUTERS_PER_DIRECTORY: usize = 2_000;

/// Directory Service's default quota "Manual snapshots: 5" per directory, the bound
/// `SnapshotLimitExceededException` reports — the same bound `GetSnapshotLimits`
/// describes as `SnapshotLimits$ManualSnapshotsLimit`.
pub const MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY: usize = 5;

/// The most snapshots this emulator keeps in total, across every directory.
///
/// NOT an AWS quota: a resource guard. [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`] bounds one
/// directory's snapshots and `DeleteDirectory` drops the snapshots of the directory it
/// deletes, so this is the absolute ceiling of the snapshot store — exactly as many as a
/// Region's worth of directories can hold at once — and it is what keeps a
/// create-directory / snapshot / delete-directory cycle from growing the store without
/// end. Past it `CreateSnapshot` is refused.
pub const MAX_SNAPSHOTS: usize = MAX_DIRECTORIES * MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY;

/// The `TopicStatus` of an association this emulator has just made.
///
/// The first value of the modelled `TopicStatus` enum, and the one the `Status`
/// member ("The topic registration status") reports for a registration that holds:
/// the other values describe a topic the SNS data plane this emulator does not have
/// would have to report on.
pub const TOPIC_STATUS_REGISTERED: &str = "Registered";

/// The `DataAccessStatus` of a directory nothing has enabled the Directory Service Data
/// API for: the `Disabled` value of the modelled `DataAccessStatus` enum
/// (`smithy_query(query="shape", shape="DataAccessStatus")` answers
/// `["Disabled", "Disabling", "Enabled", "Enabling", "Failed"]`), and the status
/// `DescribeDirectoryDataAccess` therefore reports for every freshly created directory.
pub const DATA_ACCESS_DISABLED: &str = "Disabled";

/// The `DataAccessStatus` a directory carries between `EnableDirectoryDataAccess` and
/// [`TRANSITION_SPAN`]: the modelled enum's `Enabling` value.
///
/// The enable answers before data access is in service — the enum carries both the
/// transient `Enabling` and the settled `Enabled` — so the move to
/// [`DATA_ACCESS_ENABLED`] is owed by the clock, on the same span a create's stage
/// transition uses.
pub const DATA_ACCESS_ENABLING: &str = "Enabling";

/// The `DataAccessStatus` a directory reaches once the enable's [`TRANSITION_SPAN`] has
/// elapsed: the modelled enum's `Enabled` value.
pub const DATA_ACCESS_ENABLED: &str = "Enabled";

/// The `TrustState` a trust relationship carries between `CreateTrust` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `TrustState` enum
/// (`smithy_query(query="shape",shape="TrustState")`).
///
/// `CreateTrust` "initiates the creation of the Amazon Web Services side of a trust
/// relationship", so it answers while the relationship is still being established and the
/// move to [`TRUST_STATE_CREATED`] is owed by the clock rather than by any later call, on
/// the same span a directory's stage transition uses.
pub const TRUST_STATE_CREATING: &str = "Creating";

/// The `TrustState` a trust relationship reaches once its [`TRANSITION_SPAN`] has
/// elapsed: the `Created` value of the modelled `TrustState` enum.
pub const TRUST_STATE_CREATED: &str = "Created";

/// The `TrustState` a trust relationship carries between `VerifyTrust` and
/// [`TRANSITION_SPAN`]: the `Verifying` value of the modelled `TrustState` enum.
///
/// `VerifyTrust` starts the verification rather than reporting its outcome — the enum
/// carries both the transient `Verifying` and the settled `Verified` — so the move to
/// [`TRUST_STATE_VERIFIED`] is owed by the clock, on the same span a create's transition
/// uses.
pub const TRUST_STATE_VERIFYING: &str = "Verifying";

/// The `TrustState` a trust relationship reaches once its verification's
/// [`TRANSITION_SPAN`] has elapsed: the `Verified` value of the modelled `TrustState`
/// enum.
pub const TRUST_STATE_VERIFIED: &str = "Verified";

/// The `TrustState` a trust relationship carries between `DeleteTrust` and
/// [`TRANSITION_SPAN`], after which the relationship is gone: the `Deleting` value of the
/// modelled `TrustState` enum.
///
/// `DeleteTrust` answers while the Amazon Web Services side is still being torn down, so a
/// caller acting straight after it still sees the trust in this state, and the removal is
/// owed by the clock rather than by any later call.
pub const TRUST_STATE_DELETING: &str = "Deleting";

/// The `TrustType` a request that names none gets: the `Forest` value of the modelled
/// `TrustType` enum, from `CreateTrustRequest$TrustType`'s own documentation — "The trust
/// relationship type. `Forest` is the default."
pub const TRUST_TYPE_FOREST: &str = "Forest";

/// The `SelectiveAuth` a trust relationship whose request asked for none carries: the
/// `Disabled` value of the modelled `SelectiveAuth` enum.
///
/// `CreateTrustRequest$SelectiveAuth` is the "optional parameter to enable selective
/// authentication for the trust", and `Trust$SelectiveAuth` reports the "current state of
/// selective authentication" — a state every stored trust has, so a request that enabled
/// nothing leaves it at the enum's not-enabled value.
pub const SELECTIVE_AUTH_DISABLED: &str = "Disabled";

/// The ARN service segment of an Amazon SNS topic, so a reported `TopicArn` is built
/// through [`::engine::emulator_arn`] rather than written as a literal.
const SNS_ARN_SERVICE: &str = "sns";

/// The Region an ARN this service reports carries, and the Region a request that acts
/// per Region is taken to name, when the request carries no sigv4 credential scope to
/// read one from — the default `emulator_arn` documents for generated emulators.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most Amazon SNS topics this emulator will associate with one directory.
///
/// NOT an AWS quota: a resource guard. `RegisterEventTopic` is the one write a caller
/// can repeat with a fresh name forever, so the per-directory index carries its own
/// bound, enforced at the single insert site in `register_event_topic.rs`.
pub const MAX_EVENT_TOPICS_PER_DIRECTORY: usize = 50;

/// The `ReplicationScope` every stored conditional forwarder carries: the only value the
/// modelled `ReplicationScope` enum declares, as the
/// `ConditionalForwarder$ReplicationScope` member's own documentation states — "The only
/// allowed value is `Domain`, which will replicate the conditional forwarder to all of
/// the domain controllers for your Amazon Web Services directory."
pub const REPLICATION_SCOPE_DOMAIN: &str = "Domain";

/// The most conditional forwarders this emulator will store on one directory.
///
/// NOT an AWS quota: a resource guard. `CreateConditionalForwarder` is a write a caller
/// can repeat with a fresh remote domain name forever, so the per-directory index carries
/// its own bound, enforced at the single insert site in
/// `create_conditional_forwarder.rs`.
pub const MAX_CONDITIONAL_FORWARDERS_PER_DIRECTORY: usize = 50;

/// The most `RemoteDomainNames` entries a single `DescribeConditionalForwarders` will
/// resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most
/// [`MAX_CONDITIONAL_FORWARDERS_PER_DIRECTORY`] forwarders, so a longer list can only
/// repeat names — and it is what keeps the caller-chosen filter from being allocated and
/// scanned unbounded.
pub const MAX_REMOTE_DOMAIN_NAMES_PER_REQUEST: usize = 100;

/// The most `TopicNames` entries a single `DescribeEventTopics` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most [`MAX_EVENT_TOPICS_PER_DIRECTORY`]
/// associations, so a longer list can only repeat names — and it is what keeps the
/// caller-chosen filter from being allocated and scanned unbounded.
pub const MAX_TOPIC_NAMES_PER_REQUEST: usize = 100;

/// The most entries this emulator keeps in the alias reservation index.
///
/// NOT an AWS quota: a resource guard. An alias "cannot be deleted or reused", so an
/// entry outlives the directory it was created for and the index is the one store here
/// that a caller can grow without deleting anything (create, alias, delete, repeat).
/// Stated as its own figure rather than derived from a quota, so it does not move when a
/// published per-category quota does: it is well above the [`MAX_DIRECTORIES`]
/// directories a Region can hold at once, and past it `CreateAlias` is refused.
pub const MAX_RESERVED_ALIASES: usize = 100;

/// The most `DirectoryIds` entries a single `DescribeDirectories` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a Region holds at most [`MAX_DIRECTORIES`] directories, so a longer
/// list can only repeat ids — and it is what keeps the caller-chosen list from being
/// allocated and scanned unbounded.
pub const MAX_DIRECTORY_IDS_PER_REQUEST: usize = 100;

/// The most `SnapshotIds` entries a single `DescribeSnapshots` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — an account holds at most
/// [`MAX_DIRECTORIES`] × [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`] snapshots, so a longer
/// list can only repeat ids — and it is what keeps the caller-chosen list from being
/// allocated and scanned unbounded.
pub const MAX_SNAPSHOT_IDS_PER_REQUEST: usize = 100;

/// The most trust relationships this emulator will store on one directory.
///
/// NOT an AWS quota: a resource guard. `CreateTrust` is a write a caller can repeat with
/// a fresh remote domain name forever, so the trust store carries its own per-directory
/// bound, enforced at the single insert site in `create_trust.rs`.
pub const MAX_TRUSTS_PER_DIRECTORY: usize = 50;

/// The most `TrustIds` entries a single `DescribeTrusts` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most [`MAX_TRUSTS_PER_DIRECTORY`] trusts, so a
/// longer list can only repeat ids — and it is what keeps the caller-chosen filter from
/// being allocated and scanned unbounded.
pub const MAX_TRUST_IDS_PER_REQUEST: usize = 100;

/// The page size a paginated operation uses when the request declares none.
///
/// NOT an AWS quota: `Limit`'s own documentation says "If this value is zero, the
/// maximum number of items is specified by the limitations of the operation", and this
/// is that limitation for this emulator. It is above [`MAX_DIRECTORIES`] and
/// [`MAX_TAGS`], so an unpaginated caller always sees everything stored.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The longest `NextToken` this service will decode.
///
/// NOT an AWS quota: a resource guard. Every token this service issues is far shorter
/// (the kind tag, a creation rank or a tag key, and the listing's filter, hex-encoded),
/// so a longer one cannot be one of ours and is refused BEFORE anything is allocated
/// from it.
pub const MAX_NEXT_TOKEN_CHARS: usize = 1024;

/// The version tag every [`encode_cursor`] token carries, so a token minted by an older
/// build is refused rather than misread.
const CURSOR_VERSION: &str = "v1";

/// One IP address block `AddIpRoutes` added to a directory, as `ListIpRoutes` reports
/// it.
///
/// A block is identified by the CIDR it routes — exactly one of `CidrIp` (IPv4) or
/// `CidrIpv6` — which is why [`Self::block`] is what the per-directory map is keyed by:
/// a second `AddIpRoutes` naming a block already routed is a duplicate, not an update.
#[derive(Debug, Clone)]
pub struct IpRouteRecord {
    /// The IPv4 block this route carries, when the caller gave one.
    pub cidr_ip: Option<String>,
    /// The IPv6 block this route carries, when the caller gave one.
    pub cidr_ipv6: Option<String>,
    /// The caller's description of the block, absent when the request carried none.
    pub description: Option<String>,
    /// One of the modelled `IpRouteStatusMsg` values.
    pub status: String,
    /// When the status transition the add owes falls due; `None` once there is none
    /// left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// When `AddIpRoutes` added the block, as `AddedDateTime` reports it.
    pub added_at: SystemTime,
}

impl IpRouteRecord {
    /// The address block this route routes: the IPv4 CIDR when it carries one, the
    /// IPv6 CIDR otherwise. This is the identity of the route within a directory.
    pub fn block(&self) -> &str {
        self.cidr_ip
            .as_deref()
            .or(self.cidr_ipv6.as_deref())
            .unwrap_or_default()
    }

    /// This route as the modelled `IpRouteInfo` of `directory_id`.
    ///
    /// `IpRouteStatusReason` ("The reason for the IpRouteStatusMsg") stays absent: a
    /// route this emulator adds never fails, so there is no reason to report.
    pub fn to_info(&self, directory_id: &str) -> IpRouteInfo {
        IpRouteInfo {
            directory_id: Some(directory_id.to_string()),
            cidr_ip: self.cidr_ip.clone(),
            cidr_ipv6: self.cidr_ipv6.clone(),
            ip_route_status_msg: Some(self.status.clone()),
            added_date_time: Some(as_timestamp(self.added_at)),
            ip_route_status_reason: None,
            description: self.description.clone(),
        }
    }
}

/// The LDAPS security recorded for one `LDAPSType` of a directory, as
/// `DescribeLDAPSSettings` reports it.
///
/// `EnableLDAPS` and `DisableLDAPS` (both still scaffolded) are what record and
/// change one, so a directory nothing has enabled LDAPS for carries no entry at all —
/// which is what `DescribeLDAPSSettings` then reports: an empty list.
#[derive(Debug, Clone)]
pub struct LdapsSetting {
    /// One of the modelled `LDAPSStatus` values.
    pub status: String,
    /// The modelled `LDAPSStatusReason`, "Describes a state change for LDAPS".
    pub status_reason: Option<String>,
    /// When this setting last changed, as `LastUpdatedDateTime` reports it.
    pub last_updated: SystemTime,
}

impl LdapsSetting {
    /// This setting as the modelled `LDAPSSettingInfo`.
    pub fn to_info(&self) -> LDAPSSettingInfo {
        LDAPSSettingInfo {
            ldaps_status: Some(self.status.clone()),
            ldaps_status_reason: self.status_reason.clone(),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
        }
    }
}

/// The client authentication recorded for one `ClientAuthenticationType` of a
/// directory, as `DescribeClientAuthenticationSettings` reports it.
///
/// `EnableClientAuthentication` and `DisableClientAuthentication` (both still
/// scaffolded) are what record and change one, so a directory nothing has configured
/// client authentication for carries no entry and reports an empty list.
#[derive(Debug, Clone)]
pub struct ClientAuthenticationSetting {
    /// One of the modelled `ClientAuthenticationStatus` values.
    pub status: String,
    /// When this setting last changed, as `LastUpdatedDateTime` reports it.
    pub last_updated: SystemTime,
}

impl ClientAuthenticationSetting {
    /// This setting as the modelled `ClientAuthenticationSettingInfo` for
    /// `authentication_type`.
    pub fn to_info(&self, authentication_type: &str) -> ClientAuthenticationSettingInfo {
        ClientAuthenticationSettingInfo {
            r#type: Some(authentication_type.to_string()),
            status: Some(self.status.clone()),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
        }
    }
}

/// One stored computer account: the Active Directory computer object `CreateComputer`
/// created in a directory.
///
/// Named `ComputerRecord` rather than `Computer` so it does not collide with the modelled
/// `Computer` shape [`Self::to_computer`] renders it into.
///
/// The one-time join `Password` the create was called with is deliberately NOT a field:
/// the model marks it `@sensitive`, no operation reports it back, and state this emulator
/// does not need is state it must not hold \u2014 the same reason the directory record holds
/// no administrator password.
#[derive(Debug, Clone)]
pub struct ComputerRecord {
    /// The account's SID, as the modelled `Computer$ComputerId` reports it: this
    /// directory's [`Directory::domain_sid`] plus the RID the directory handed out. See
    /// [`FIRST_COMPUTER_RID`].
    pub computer_id: String,
    /// The account name as the request spelled it. The lookup key folds it to lower case
    /// (see [`Directory::computers`]); this is what is reported back.
    pub computer_name: String,
    /// The organizational unit the account was placed in, when the create named one.
    /// `None` means the directory's own default computers container, which is what
    /// `OrganizationalUnitDistinguishedName`'s documentation describes as the placement
    /// when the member is absent.
    pub organizational_unit_distinguished_name: Option<String>,
    /// The LDAP attributes the create applied to the account, in the order the request
    /// listed them \u2014 which is what the modelled `Computer$ComputerAttributes` ("the LDAP
    /// attributes that belong to the computer account") reports. Nothing else is invented
    /// into it: an attribute this emulator did not apply is an attribute no caller set.
    pub attributes: Vec<Attribute>,
    /// When the account was created.
    pub created_at: SystemTime,
}

impl ComputerRecord {
    /// This account as the modelled `Computer`.
    pub fn to_computer(&self) -> Computer {
        Computer {
            computer_id: Some(self.computer_id.clone()),
            computer_name: Some(self.computer_name.clone()),
            // Absent rather than an empty list when the create applied no attributes, so
            // an account nothing was applied to does not report a list that was never
            // recorded.
            computer_attributes: if self.attributes.is_empty() {
                None
            } else {
                Some(self.attributes.clone())
            },
        }
    }
}

/// Separates the creation rank a cursor resumes after from the filter it was issued
/// under, in every paginated operation of this service. Not a character a `DirectoryId`,
/// a `SnapshotId` or a tag key the listings filter on can contain, so the two halves are
/// unambiguous — and it lives here, beside [`encode_cursor`], because the listings must
/// agree on it for a token to mean one thing.
pub const CURSOR_SEPARATOR: char = '|';

/// Separates the parts of one listing's filter inside a cursor. Not a character a
/// `DirectoryId`, a `SnapshotId` or a tag key can contain, so the parts stay
/// unambiguous.
const CURSOR_FILTER_SEPARATOR: char = '/';

/// How a listing records the filter its page was issued under, inside the cursor: its
/// parts joined by [`CURSOR_FILTER_SEPARATOR`], each part being that member as the
/// request listed it (or the empty string when the request omitted it).
///
/// One encoding for every paginated operation, so the filter half of a token cannot
/// drift between listings: a later page asked for under a different filter decodes to a
/// different string and is refused with [`INVALID_NEXT_TOKEN`].
pub fn cursor_filter(parts: &[&str]) -> String {
    parts.join(&CURSOR_FILTER_SEPARATOR.to_string())
}

/// One stored association between a directory and an Amazon SNS topic: what
/// `RegisterEventTopic` persisted and `DescribeEventTopics` reports.
#[derive(Debug, Clone)]
pub struct EventTopicRegistration {
    pub topic_name: String,
    /// The topic's ARN, built once at registration through [`sns_topic_arn`] so every
    /// later read reports the same one.
    pub topic_arn: String,
    /// When the association was made — the `CreatedDateTime` member's "the date and
    /// time of when you associated your directory with the Amazon SNS topic".
    pub created_at: SystemTime,
    /// One of the modelled `TopicStatus` values; [`TOPIC_STATUS_REGISTERED`] here.
    pub status: String,
}

impl EventTopicRegistration {
    /// This association as the modelled `EventTopic`, reported for `directory_id`.
    ///
    /// The one place a stored registration is rendered, so `DescribeEventTopics`
    /// reports exactly what was persisted.
    pub fn to_event_topic(&self, directory_id: &str) -> EventTopic {
        EventTopic {
            directory_id: Some(directory_id.to_string()),
            topic_name: Some(self.topic_name.clone()),
            topic_arn: Some(self.topic_arn.clone()),
            created_date_time: Some(as_timestamp(self.created_at)),
            status: Some(self.status.clone()),
        }
    }
}

/// One stored conditional forwarder of a directory: what `CreateConditionalForwarder`
/// persisted and `DescribeConditionalForwarders` reports.
///
/// A forwarder is identified by the remote domain it points to, which is why the
/// per-directory index is keyed by [`Directory::conditional_forwarder_key`] of that name
/// rather than by this record.
#[derive(Debug, Clone)]
pub struct ConditionalForwarderRecord {
    /// The remote domain name as the request spelled it, so the report gives the caller
    /// back its own spelling even though the lookup folds it.
    pub remote_domain_name: String,
    /// The IPv4 addresses of the remote DNS server the forwarder points to, in the order
    /// the request listed them.
    pub dns_ip_addrs: Vec<String>,
    /// The IPv6 addresses of the remote DNS server the forwarder points to, in the order
    /// the request listed them.
    pub dns_ipv6_addrs: Vec<String>,
    /// One of the modelled `ReplicationScope` values; [`REPLICATION_SCOPE_DOMAIN`] here.
    pub replication_scope: String,
}

impl ConditionalForwarderRecord {
    /// This forwarder as the modelled `ConditionalForwarder`.
    ///
    /// The one place a stored forwarder is rendered, so every operation that reports one
    /// reports exactly what was persisted.
    pub fn to_conditional_forwarder(&self) -> ConditionalForwarder {
        ConditionalForwarder {
            remote_domain_name: Some(self.remote_domain_name.clone()),
            dns_ip_addrs: optional_list(&self.dns_ip_addrs),
            dns_ipv6_addrs: optional_list(&self.dns_ipv6_addrs),
            replication_scope: Some(self.replication_scope.clone()),
        }
    }
}

/// One stored directory: what `CreateDirectory` persisted and `DescribeDirectories`
/// reports.
#[derive(Debug, Clone)]
pub struct Directory {
    pub directory_id: String,
    pub name: String,
    pub short_name: Option<String>,
    pub description: Option<String>,
    /// Only a Simple AD directory has a size (`Small`/`Large`), so this stays `None`
    /// for a Managed Microsoft AD directory, which carries an [`Self::edition`]
    /// instead.
    pub size: Option<String>,
    /// Only AWS Managed Microsoft AD has an edition, so this stays `None` for a
    /// Simple AD directory and `CreateMicrosoftAD` sets it.
    pub edition: Option<String>,
    /// One of the modelled `DirectoryType` values: [`TYPE_SIMPLE_AD`] for a directory
    /// `CreateDirectory` persisted, [`TYPE_MICROSOFT_AD`] for one `CreateMicrosoftAD`
    /// persisted.
    pub directory_type: String,
    /// One of the modelled `DirectoryStage` values.
    pub stage: String,
    /// When the stage transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub stage_due_at: Option<SystemTime>,
    pub launch_time: SystemTime,
    pub stage_last_updated: SystemTime,
    /// The alias `CreateAlias` assigned, if any. `None` means the directory id stands
    /// in for it, as the `Alias` member's documentation describes.
    pub alias: Option<String>,
    pub vpc_settings: Option<DirectoryVpcSettingsDescription>,
    pub network_type: Option<String>,
    pub sso_enabled: bool,
    /// Reported only for AWS Managed Microsoft AD, so it stays `None` for Simple AD:
    /// `CreateMicrosoftAD` sets it to [`MICROSOFT_AD_DOMAIN_CONTROLLERS`] and
    /// `UpdateNumberOfDomainControllers` (still scaffolded) is what changes it.
    pub desired_number_of_domain_controllers: Option<i32>,
    /// This directory's tags, keyed by tag key: a second `AddTagsToResource` with a key
    /// already present overwrites that key's value.
    pub tags: BTreeMap<String, String>,
    /// The Region this directory was created in: the one `RegionType` `Primary` names,
    /// which only `DeleteDirectory` removes. Read from the create request's sigv4
    /// credential scope, the only Region a request carries.
    pub primary_region: String,
    /// The Regions this directory is replicated to beside [`Self::primary_region`] —
    /// the `Additional` ones. `AddRegion` (still scaffolded) is what adds one, and
    /// `RemoveRegion` is what removes one.
    pub additional_regions: BTreeSet<String>,
    /// The IP address blocks `AddIpRoutes` routed to this directory, keyed by
    /// [`IpRouteRecord::block`].
    pub ip_routes: BTreeMap<String, IpRouteRecord>,
    /// The LDAPS security recorded for this directory, keyed by the modelled
    /// `LDAPSType`; `DescribeLDAPSSettings` reports it.
    pub ldaps_settings: BTreeMap<String, LdapsSetting>,
    /// The client authentication recorded for this directory, keyed by the modelled
    /// `ClientAuthenticationType`; `DescribeClientAuthenticationSettings` reports it.
    pub client_authentication_settings: BTreeMap<String, ClientAuthenticationSetting>,
    /// The Amazon SNS topics `RegisterEventTopic` associated with this directory, keyed
    /// by topic name — a directory publishes to a given topic at most once, and the
    /// associations live on the directory record, so deleting the directory takes its
    /// associations with it.
    pub event_topics: BTreeMap<String, EventTopicRegistration>,
    /// The conditional forwarders `CreateConditionalForwarder` created on this directory,
    /// keyed by [`Directory::conditional_forwarder_key`] — a directory points at a given
    /// remote domain at most once. The forwarders live on the directory record, so
    /// deleting the directory takes them with it, and two directories can each point at
    /// the same remote domain.
    pub conditional_forwarders: BTreeMap<String, ConditionalForwarderRecord>,
    /// The computer accounts `CreateComputer` created in this directory, keyed by the
    /// account name folded to lower case: an Active Directory account name is
    /// case-insensitive, so that fold is what makes the duplicate check agree with the
    /// directory it emulates. The accounts live on the directory record, so deleting the
    /// directory takes them with it, and two directories can each hold an account of the
    /// same name.
    pub computers: BTreeMap<String, ComputerRecord>,
    /// The next RID this directory's domain hands to an account created in it, starting
    /// at [`FIRST_COMPUTER_RID`]. A pool rather than `computers.len()`: a RID a domain has
    /// handed out is never handed out again.
    pub next_computer_rid: u32,
    /// Whether this directory's data is reachable through the Directory Service Data
    /// API, as `DescribeDirectoryDataAccess` reports it: one of the modelled
    /// `DataAccessStatus` values, [`DATA_ACCESS_DISABLED`] until
    /// `EnableDirectoryDataAccess` records otherwise.
    pub data_access_status: String,
    /// When the data-access status transition an enable owes falls due; `None` once
    /// there is none left to make. See [`TRANSITION_SPAN`].
    pub data_access_due_at: Option<SystemTime>,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`]. A
    /// listing pages over directories sorted by this, and its opaque `NextToken`
    /// carries the rank the next page resumes after, so a page boundary survives a
    /// directory being deleted between pages.
    pub seq: u64,
}

impl Directory {
    /// The `Alias` this directory reports: the alias `CreateAlias` assigned, or the
    /// directory id when none was.
    ///
    /// From the `Alias` member's documentation: "If no alias exists, the alias is the
    /// directory identifier, such as `d-XXXXXXXXXX`."
    pub fn effective_alias(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.directory_id)
    }

    /// The `AccessUrl` this directory reports, derived from [`Self::effective_alias`].
    ///
    /// From the `AccessUrl` member's documentation: "The access URL for the directory,
    /// such as `http://<alias>.awsapps.com`. If no alias exists, `<alias>` is the
    /// directory identifier".
    pub fn access_url(&self) -> String {
        format!(
            "http://{alias}{ACCESS_URL_SUFFIX}",
            alias = self.effective_alias()
        )
    }

    /// This directory's associations as the modelled `EventTopics` list, in topic-name
    /// order, narrowed to `wanted` when the request asked for particular topic names.
    pub fn event_topic_list(&self, wanted: Option<&BTreeSet<&String>>) -> Vec<EventTopic> {
        self.event_topics
            .values()
            .filter(|registration| {
                wanted.is_none_or(|names| names.contains(&registration.topic_name))
            })
            .map(|registration| registration.to_event_topic(&self.directory_id))
            .collect()
    }

    /// The key [`Self::conditional_forwarders`] holds `remote_domain_name` under:
    /// [`domain_key`] of that name, so a lookup agrees with DNS.
    pub fn conditional_forwarder_key(remote_domain_name: &str) -> String {
        domain_key(remote_domain_name)
    }

    /// This directory's conditional forwarders as the modelled `ConditionalForwarders`
    /// list, in remote-domain order, narrowed to `wanted` when the request asked for
    /// particular remote domains. `wanted` holds the domains folded through
    /// [`Self::conditional_forwarder_key`], so a filter matches a stored forwarder the
    /// same way a second create of it would.
    pub fn conditional_forwarder_list(
        &self,
        wanted: Option<&BTreeSet<String>>,
    ) -> Vec<ConditionalForwarder> {
        self.conditional_forwarders
            .iter()
            .filter(|(key, _)| wanted.is_none_or(|keys| keys.contains(*key)))
            .map(|(_, record)| record.to_conditional_forwarder())
            .collect()
    }

    /// This directory's tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Whether this directory exists in `region` at all: its primary Region, or one of
    /// the Regions it is replicated to.
    pub fn exists_in_region(&self, region: &str) -> bool {
        self.primary_region == region || self.additional_regions.contains(region)
    }

    /// This directory's domain SID: [`DOMAIN_SID_PREFIX`] plus the three sub-authorities
    /// that identify one Active Directory domain.
    ///
    /// Every account of a domain carries the domain's SID, so the three sub-authorities
    /// are derived from the directory's own id rather than minted: they are the same for
    /// every account of this directory, stable for the life of the record, and different
    /// for another directory, because the derivation below is injective in the id's forty
    /// bits (each sub-authority is a bijection of them modulo its own odd multiplier) and
    /// the ids themselves never repeat
    /// ([`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_directory_id`]).
    pub fn domain_sid(&self) -> String {
        // The id's ten hex digits, the width the modelled `DirectoryId` pattern
        // `^d-[0-9a-f]{10}$` declares. A record is only ever stored under a minted id, so
        // the fallback is unreachable and merely keeps this total.
        let seed = self
            .directory_id
            .strip_prefix("d-")
            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
            .unwrap_or_default();
        // A sub-authority is a 32-bit number, and never zero: a domain SID's
        // sub-authorities are non-zero in every SID Windows renders.
        let sub_authority = |multiplier: u64, offset: u64| -> u32 {
            let span = u64::from(u32::MAX);
            (seed.wrapping_mul(multiplier).wrapping_add(offset) % span + 1) as u32
        };
        format!(
            "{DOMAIN_SID_PREFIX}-{first}-{second}-{third}",
            first = sub_authority(2_654_435_761, 1_779_033_703),
            second = sub_authority(2_246_822_519, 3_144_134_277),
            third = sub_authority(3_266_489_917, 1_013_904_242)
        )
    }

    /// The domain name components a distinguished name in this directory's domain ends
    /// with: the DNS labels of the directory's own `Name`, as `dc=` relative
    /// distinguished names.
    ///
    /// The directory's `Name` is its fully qualified domain name (`CreateDirectory`'s
    /// documentation: "The fully qualified name for the directory, such as
    /// `corp.example.com`"), and a DN in that domain ends with one `dc=` component per
    /// label, so this is what an `OrganizationalUnitDistinguishedName` is checked against.
    /// Folded to lower case, because a DN's attribute values are compared
    /// case-insensitively.
    pub fn domain_name_labels(&self) -> Vec<String> {
        self.name
            .split('.')
            .filter(|label| !label.is_empty())
            .map(|label| label.to_lowercase())
            .collect()
    }

    /// The key [`Self::computers`] holds `computer_name` under: the name folded to lower
    /// case, so a lookup agrees with Active Directory's case-insensitive account names.
    pub fn computer_key(computer_name: &str) -> String {
        computer_name.to_lowercase()
    }

    /// Hand out the next computer account SID of this directory's domain: its
    /// [`Self::domain_sid`] plus the next RID from the pool.
    ///
    /// MUTATING: the RID is consumed, so no two accounts of a directory can carry the
    /// same `ComputerId` even after one of them is gone.
    pub fn mint_computer_sid(&mut self) -> String {
        let rid = self.next_computer_rid.max(FIRST_COMPUTER_RID);
        self.next_computer_rid = rid.saturating_add(1);
        format!("{domain}-{rid}", domain = self.domain_sid())
    }

    /// This directory as the modelled `DirectoryDescription`.
    ///
    /// The members this slice has no record behind — the DNS addresses of directory
    /// servers, RADIUS, sharing, regions, the owner description — are left absent
    /// rather than invented: the operations that would populate them are still
    /// scaffolded.
    pub fn to_description(&self) -> DirectoryDescription {
        DirectoryDescription {
            directory_id: Some(self.directory_id.clone()),
            name: Some(self.name.clone()),
            short_name: self.short_name.clone(),
            size: self.size.clone(),
            edition: self.edition.clone(),
            alias: Some(self.effective_alias().to_string()),
            access_url: Some(self.access_url()),
            description: self.description.clone(),
            stage: Some(self.stage.clone()),
            launch_time: Some(as_timestamp(self.launch_time)),
            stage_last_updated_date_time: Some(as_timestamp(self.stage_last_updated)),
            r#type: Some(self.directory_type.clone()),
            vpc_settings: self.vpc_settings.clone(),
            sso_enabled: self.sso_enabled,
            desired_number_of_domain_controllers: self.desired_number_of_domain_controllers,
            network_type: self.network_type.clone(),
            ..DirectoryDescription::default()
        }
    }
}

/// One stored directory assessment: what `StartADAssessment` persisted and
/// `DescribeADAssessment` / `ListADAssessments` report.
///
/// Named `AssessmentRecord` rather than `Assessment` so it does not collide with the
/// modelled `Assessment` shape [`Self::to_assessment`] renders it into.
#[derive(Debug, Clone)]
pub struct AssessmentRecord {
    pub assessment_id: String,
    /// The directory the assessment was started for, when `StartADAssessment` was
    /// called with a `DirectoryId`. `None` for an assessment started from an
    /// `AssessmentConfiguration`, which names no existing directory.
    pub directory_id: Option<String>,
    pub dns_name: Option<String>,
    /// The self-managed DNS servers the configuration named; empty for an assessment
    /// started from a `DirectoryId`, for which this emulator holds no self-managed DNS
    /// record.
    pub customer_dns_ips: Vec<String>,
    pub vpc_id: Option<String>,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    /// The Systems Manager managed nodes the configuration named, in the order it
    /// listed them: `StartADAssessment`'s documentation requires `InstanceIds` to keep
    /// a one-to-one positional correspondence with `CustomerDnsIps`, so the order is
    /// preserved exactly as given.
    pub self_managed_instance_ids: Vec<String>,
    /// One of the modelled `AssessmentReportType` values;
    /// [`ASSESSMENT_REPORT_TYPE_CUSTOMER`] for a caller-started assessment.
    pub report_type: String,
    /// One of the modelled `AssessmentStatus` values.
    pub status: String,
    pub status_reason: Option<String>,
    pub start_time: SystemTime,
    pub last_update: SystemTime,
    /// When the status transition the start owes falls due; `None` once the assessment
    /// has reached a terminal status. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], the
    /// same counter the directory records are stamped from. `ListADAssessments` pages
    /// over assessments sorted by this, and its opaque `NextToken` carries the rank the
    /// next page resumes after, so a page boundary survives an assessment being deleted
    /// between pages.
    pub seq: u64,
}

impl AssessmentRecord {
    /// This assessment as the modelled `Assessment`.
    ///
    /// The members this emulator has no record behind are left absent rather than
    /// invented: `StatusCode` ("a detailed status code providing additional information
    /// about the assessment state", which AWS does not document a value list for) and
    /// `Version` ("the version of the assessment framework used to evaluate your
    /// self-managed AD environment" — no framework ran, see
    /// [`ASSESSMENT_FAILED_REASON`]).
    pub fn to_assessment(&self) -> Assessment {
        Assessment {
            assessment_id: Some(self.assessment_id.clone()),
            directory_id: self.directory_id.clone(),
            dns_name: self.dns_name.clone(),
            start_time: Some(as_timestamp(self.start_time)),
            last_update_date_time: Some(as_timestamp(self.last_update)),
            status: Some(self.status.clone()),
            status_code: None,
            status_reason: self.status_reason.clone(),
            customer_dns_ips: optional_list(&self.customer_dns_ips),
            vpc_id: self.vpc_id.clone(),
            subnet_ids: optional_list(&self.subnet_ids),
            security_group_ids: optional_list(&self.security_group_ids),
            self_managed_instance_ids: optional_list(&self.self_managed_instance_ids),
            report_type: Some(self.report_type.clone()),
            version: None,
        }
    }

    /// This assessment as the modelled `AssessmentSummary`: the same record "without
    /// detailed validation results", as that shape's own documentation puts it, so every
    /// member it shares with [`Self::to_assessment`] reports the same value.
    pub fn to_summary(&self) -> AssessmentSummary {
        AssessmentSummary {
            assessment_id: Some(self.assessment_id.clone()),
            directory_id: self.directory_id.clone(),
            dns_name: self.dns_name.clone(),
            start_time: Some(as_timestamp(self.start_time)),
            last_update_date_time: Some(as_timestamp(self.last_update)),
            status: Some(self.status.clone()),
            customer_dns_ips: optional_list(&self.customer_dns_ips),
            report_type: Some(self.report_type.clone()),
        }
    }
}

/// One stored snapshot: what `CreateSnapshot` persisted and `DescribeSnapshots`
/// reports.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    /// The directory this snapshot was taken of. A snapshot does not outlive that
    /// directory: `DeleteDirectory` drops the snapshots naming the id it deletes, as
    /// the real service does.
    pub directory_id: String,
    /// The descriptive `Name` the create was called with, if any.
    pub name: Option<String>,
    /// One of the modelled `SnapshotType` values; [`SNAPSHOT_TYPE_MANUAL`] for a
    /// snapshot a caller asked for.
    pub snapshot_type: String,
    /// One of the modelled `SnapshotStatus` values.
    pub status: String,
    /// When the status transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// "The date and time that the snapshot was taken", the `StartTime` member reports.
    pub start_time: SystemTime,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl SnapshotRecord {
    /// This snapshot as the modelled `Snapshot` structure.
    pub fn to_snapshot(&self) -> Snapshot {
        Snapshot {
            directory_id: Some(self.directory_id.clone()),
            snapshot_id: Some(self.snapshot_id.clone()),
            r#type: Some(self.snapshot_type.clone()),
            name: self.name.clone(),
            status: Some(self.status.clone()),
            start_time: Some(as_timestamp(self.start_time)),
        }
    }
}

/// One stored trust relationship: what `CreateTrust` persisted and `DescribeTrusts`
/// reports.
///
/// A trust is a relationship between ONE Managed Microsoft AD directory and ONE external
/// domain, so a directory points at a given remote domain at most once — which is what
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::trust_of`] resolves. The
/// `TrustPassword` the create was called with is deliberately not a field: the model marks
/// it `@sensitive`, no operation reports it back, and state this emulator does not need is
/// state it must not hold.
#[derive(Debug, Clone)]
pub struct TrustRecord {
    pub trust_id: String,
    /// The Managed Microsoft AD directory this is the Amazon Web Services side of. A
    /// trust does not outlive that directory: `DeleteDirectory` drops the trusts naming
    /// the id it deletes.
    pub directory_id: String,
    /// The FQDN of the external domain, as the request spelled it, so the report gives the
    /// caller back its own spelling even though the lookup folds it.
    pub remote_domain_name: String,
    /// One of the modelled `TrustType` values; [`TRUST_TYPE_FOREST`] when the request
    /// named none.
    pub trust_type: String,
    /// One of the modelled `TrustDirection` values, as the request required.
    pub trust_direction: String,
    /// One of the modelled `SelectiveAuth` values; [`SELECTIVE_AUTH_DISABLED`] when the
    /// request named none.
    pub selective_auth: String,
    /// One of the modelled `TrustState` values.
    pub trust_state: String,
    /// When the state transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub trust_state_due_at: Option<SystemTime>,
    /// "The date and time that the trust relationship was created", the `CreatedDateTime`
    /// member reports.
    pub created_at: SystemTime,
    /// "The date and time that the trust relationship was last updated", the
    /// `LastUpdatedDateTime` member reports.
    pub last_updated: SystemTime,
    /// "The date and time that the TrustState was last updated", the
    /// `StateLastUpdatedDateTime` member reports.
    pub state_last_updated: SystemTime,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl TrustRecord {
    /// This trust as the modelled `Trust` structure.
    ///
    /// The one place a stored trust is rendered, so every operation that reports one
    /// reports exactly what was persisted. `TrustStateReason` ("the reason for the
    /// TrustState") stays absent: it explains a state this emulator never records, and a
    /// trust that is being created or is created has no reason to give.
    pub fn to_trust(&self) -> Trust {
        Trust {
            directory_id: Some(self.directory_id.clone()),
            trust_id: Some(self.trust_id.clone()),
            remote_domain_name: Some(self.remote_domain_name.clone()),
            trust_type: Some(self.trust_type.clone()),
            trust_direction: Some(self.trust_direction.clone()),
            trust_state: Some(self.trust_state.clone()),
            created_date_time: Some(as_timestamp(self.created_at)),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
            state_last_updated_date_time: Some(as_timestamp(self.state_last_updated)),
            trust_state_reason: None,
            selective_auth: Some(self.selective_auth.clone()),
        }
    }
}

/// How this service compares two spellings of ONE remote domain name: the name folded to
/// lower case with the trailing root label dropped, so a lookup agrees with DNS, where a
/// domain name is compared case-insensitively and the fully qualified `example.com.` names
/// the same domain as `example.com` (the modelled `RemoteDomainName` pattern
/// `^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+[.]?$` admits both forms and either case).
///
/// One function, so the conditional forwarder index and the trust store cannot disagree
/// about which requests name the same external domain.
pub fn domain_key(remote_domain_name: &str) -> String {
    remote_domain_name.trim_end_matches('.').to_lowercase()
}

/// A stored list as an optional modelled list: absent when this emulator holds no
/// entries for it, so an empty list is never reported as though one had been recorded.
fn optional_list(values: &[String]) -> Option<Vec<String>> {
    if values.is_empty() {
        None
    } else {
        Some(values.to_vec())
    }
}

/// Per-service state for com-amazonaws-directoryservice-directoryservice-20150416. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Every stored directory, keyed by its `DirectoryId`.
    pub directories: BTreeMap<String, Directory>,
    /// Every stored directory assessment, keyed by its `AssessmentId`.
    pub assessments: BTreeMap<String, AssessmentRecord>,
    /// Every stored snapshot, keyed by its `SnapshotId`. A snapshot names the directory
    /// it was taken of, so this is one store rather than one per directory.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// Every stored trust relationship, keyed by its `TrustId`. A trust names the
    /// directory it belongs to, and `DescribeTrusts`/`VerifyTrust`/`DeleteTrust` all
    /// resolve one by trust id alone, so this is one store rather than one per directory.
    pub trusts: BTreeMap<String, TrustRecord>,
    /// Which directory owns which alias. An alias is unique across the account, so
    /// this is what `CreateAlias` checks before assigning one. An entry OUTLIVES the
    /// directory it was created for: `CreateAlias`' documentation says an alias
    /// "cannot be deleted or reused", so `DeleteDirectory` leaves it reserved — which
    /// is why this index carries its own cap, [`MAX_RESERVED_ALIASES`], enforced at
    /// the single insert site in `create_alias.rs`.
    pub aliases: BTreeMap<String, String>,
    /// Drives [`Self::mint_directory_id`]; see there for how an id is derived from it.
    id_seed: u64,
    /// Drives [`Self::mint_assessment_id`], the way [`Self::id_seed`] drives the
    /// directory ids.
    assessment_id_seed: u128,
    /// Drives [`Self::mint_snapshot_id`], kept apart from [`Self::id_seed`] so the two
    /// id spaces advance independently.
    snapshot_id_seed: u64,
    /// Drives [`Self::mint_trust_id`], kept apart from the other seeds so the id spaces
    /// advance independently.
    trust_id_seed: u64,
    /// Drives [`Self::next_seq`]: the creation rank stamped on each directory.
    seq_counter: u64,
}

/// The number of distinct ids [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_directory_id`]
/// can render: `16^10`, the width the modelled `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// allows.
const ID_SPACE: u64 = 1 << 40;

/// Scrambles consecutive seeds across [`ID_SPACE`]. Odd, so it is coprime with
/// `16^10 = 2^40`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod ID_SPACE)` a
/// BIJECTION: distinct seeds can never mint the same id.
const ID_MULTIPLIER: u64 = 1_000_000_007;

/// The bijection's constant term, so the first id minted is not a run of leading zeros.
const ID_OFFSET: u64 = 734_291;

/// The number of distinct ids
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_assessment_id`]
/// can render: `16^18 = 2^72`, the width the modelled `AssessmentId` pattern
/// `^da-[0-9a-f]{18}$` allows. Wider than [`ID_SPACE`], which is why an assessment id
/// cannot come from [`mint_id`].
const ASSESSMENT_ID_SPACE: u128 = 1 << 72;

/// Scrambles consecutive seeds across [`ASSESSMENT_ID_SPACE`], the same construction as
/// [`ID_MULTIPLIER`]: odd, so it is coprime with `2^72` and the map is a BIJECTION.
const ASSESSMENT_ID_MULTIPLIER: u128 = 1_000_000_000_000_000_003;

/// The assessment bijection's constant term, so the first id minted is not a run of
/// leading zeros.
const ASSESSMENT_ID_OFFSET: u128 = 9_876_543_210_987;

/// Mint the next id of one id space: `<prefix>-` followed by ten lower-case hex digits,
/// the shape both the modelled `DirectoryId` (`^d-[0-9a-f]{10}$`) and `SnapshotId`
/// (`^s-[0-9a-f]{10}$`) declare.
///
/// `seed` is the id space's own counter, advanced here and mapped through the bijection
/// described on [`ID_MULTIPLIER`], so an id looks unpredictable yet never repeats within
/// a process. One function, so the derivation cannot drift between the id spaces.
fn mint_id(seed: &mut u64, prefix: char) -> String {
    *seed = seed.wrapping_add(1);
    let scrambled = seed
        .wrapping_mul(ID_MULTIPLIER)
        .wrapping_add(ID_OFFSET)
        % ID_SPACE;
    format!("{prefix}-{scrambled:010x}")
}

impl ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Mint a fresh `DirectoryId` of the shape the model declares,
    /// `^d-[0-9a-f]{10}$`.
    ///
    /// The id is derived from a counter through the bijection described on
    /// [`ID_MULTIPLIER`], so it is unpredictable-looking yet never repeats within a
    /// process.
    pub fn mint_directory_id(&mut self) -> String {
        mint_id(&mut self.id_seed, 'd')
    }

    /// Mint a fresh `SnapshotId` of the shape the model declares,
    /// `^s-[0-9a-f]{10}$`, through the same bijection as
    /// [`Self::mint_directory_id`] over its own seed.
    pub fn mint_snapshot_id(&mut self) -> String {
        mint_id(&mut self.snapshot_id_seed, 's')
    }

    /// Mint a fresh `TrustId` of the shape the model declares, `^t-[0-9a-f]{10}$`,
    /// through the same bijection as [`Self::mint_directory_id`] over its own seed.
    pub fn mint_trust_id(&mut self) -> String {
        mint_id(&mut self.trust_id_seed, 't')
    }

    /// The next creation rank. Stamped on a directory or an assessment when it is
    /// created, so a listing reports creation order even though the records are keyed
    /// by id.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }

    /// Mint a fresh `AssessmentId` of the shape the model declares,
    /// `^da-[0-9a-f]{18}$`.
    ///
    /// Derived from its own counter through the bijection described on
    /// [`ASSESSMENT_ID_MULTIPLIER`]. Eighteen hex digits do not fit [`mint_id`]'s
    /// ten-digit space, so this is the same construction over a wider one.
    pub fn mint_assessment_id(&mut self) -> String {
        self.assessment_id_seed = self.assessment_id_seed.wrapping_add(1);
        let scrambled = self
            .assessment_id_seed
            .wrapping_mul(ASSESSMENT_ID_MULTIPLIER)
            .wrapping_add(ASSESSMENT_ID_OFFSET)
            % ASSESSMENT_ID_SPACE;
        format!("da-{scrambled:018x}")
    }

    /// Materialise every status transition the clock has already made due at `now`.
    ///
    /// This is the ONLY write a reporting operation performs: the changes here were
    /// owed at `now` whether or not anybody called, because each due instant was stored
    /// when the record it belongs to was created — a directory's stage when
    /// `CreateDirectory` ran, a snapshot's status when `CreateSnapshot` ran, an IP
    /// route's status when `AddIpRoutes` ran, an assessment's status when
    /// `StartADAssessment` ran, a directory's data-access status when
    /// `EnableDirectoryDataAccess` ran.
    pub fn advance_clock(&mut self, now: SystemTime) {
        for directory in self.directories.values_mut() {
            if let Some(due) = directory.stage_due_at {
                if now >= due {
                    directory.stage = STAGE_ACTIVE.to_string();
                    directory.stage_last_updated = due;
                    directory.stage_due_at = None;
                }
            }
            for route in directory.ip_routes.values_mut() {
                let Some(due) = route.status_due_at else {
                    continue;
                };
                if now >= due {
                    route.status = IP_ROUTE_STATUS_ADDED.to_string();
                    route.status_due_at = None;
                }
            }
            // The data-access transition `EnableDirectoryDataAccess` owes: the
            // transient `Enabling` settles on `Enabled`. The mirrored transition out of
            // `Disabling` belongs to `DisableDirectoryDataAccess`, which is still
            // scaffolded, so it lands with that handler rather than as a branch nothing
            // can reach.
            if let Some(due) = directory.data_access_due_at {
                if now >= due && directory.data_access_status == DATA_ACCESS_ENABLING {
                    directory.data_access_status = DATA_ACCESS_ENABLED.to_string();
                    directory.data_access_due_at = None;
                }
            }
        }
        for snapshot in self.snapshots.values_mut() {
            let Some(due) = snapshot.status_due_at else {
                continue;
            };
            if now >= due {
                snapshot.status = SNAPSHOT_STATUS_COMPLETED.to_string();
                snapshot.status_due_at = None;
            }
        }
        // The state transitions the trust writes owe: the transient `Creating` a create
        // answers in settles on `Created`, the `Verifying` a verify answers in settles on
        // `Verified`, and a trust whose delete has fallen due is gone — a deleted
        // relationship is not one `DescribeTrusts` reports. The transition `UpdateTrust`
        // owes belongs to that handler, which is still scaffolded, so it lands with it
        // rather than as a branch nothing can reach.
        for trust in self.trusts.values_mut() {
            let Some(due) = trust.trust_state_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            let settled = match trust.trust_state.as_str() {
                TRUST_STATE_CREATING => TRUST_STATE_CREATED,
                TRUST_STATE_VERIFYING => TRUST_STATE_VERIFIED,
                // A trust being deleted has no settled state to move to: it is removed
                // below, so its due instant is left in place for that pass to find.
                _ => continue,
            };
            trust.trust_state = settled.to_string();
            trust.state_last_updated = due;
            trust.last_updated = due;
            trust.trust_state_due_at = None;
        }
        self.trusts.retain(|_, trust| {
            trust.trust_state != TRUST_STATE_DELETING
                || trust.trust_state_due_at.is_some_and(|due| now < due)
        });
        for assessment in self.assessments.values_mut() {
            let Some(due) = assessment.status_due_at else {
                continue;
            };
            if now >= due {
                assessment.status = ASSESSMENT_STATUS_FAILED.to_string();
                assessment.status_reason = Some(ASSESSMENT_FAILED_REASON.to_string());
                assessment.last_update = due;
                assessment.status_due_at = None;
            }
        }
    }

    /// How many stored directories count towards `category` — the current count
    /// `GetDirectoryLimits` reports for that counter of `DirectoryLimits`.
    pub fn directories_in_category(&self, category: DirectoryLimitCategory) -> usize {
        self.directories
            .values()
            .filter(|directory| {
                DirectoryLimitCategory::of(&directory.directory_type) == Some(category)
            })
            .count()
    }

    /// Directories in creation order — the order `DescribeDirectories` pages over.
    pub fn directories_in_creation_order(&self) -> Vec<&Directory> {
        let mut ordered: Vec<&Directory> = self.directories.values().collect();
        ordered.sort_by_key(|directory| directory.seq);
        ordered
    }

    /// Assessments in creation order — the order `ListADAssessments` pages over.
    pub fn assessments_in_creation_order(&self) -> Vec<&AssessmentRecord> {
        let mut ordered: Vec<&AssessmentRecord> = self.assessments.values().collect();
        ordered.sort_by_key(|assessment| assessment.seq);
        ordered
    }

    /// How many CUSTOMER assessments the account currently holds — what
    /// [`MAX_CUSTOMER_ASSESSMENTS`] bounds.
    pub fn customer_assessment_count(&self) -> usize {
        self.assessments
            .values()
            .filter(|assessment| assessment.report_type == ASSESSMENT_REPORT_TYPE_CUSTOMER)
            .count()
    }

    /// Snapshots in creation order — the order `DescribeSnapshots` pages over.
    pub fn snapshots_in_creation_order(&self) -> Vec<&SnapshotRecord> {
        let mut ordered: Vec<&SnapshotRecord> = self.snapshots.values().collect();
        ordered.sort_by_key(|snapshot| snapshot.seq);
        ordered
    }

    /// Trust relationships in creation order — the order `DescribeTrusts` pages over.
    pub fn trusts_in_creation_order(&self) -> Vec<&TrustRecord> {
        let mut ordered: Vec<&TrustRecord> = self.trusts.values().collect();
        ordered.sort_by_key(|trust| trust.seq);
        ordered
    }

    /// The trust `directory_id` already holds with `remote_domain_name`, if any: a
    /// directory is one side of at most one trust with a given external domain, so this is
    /// what a second `CreateTrust` for it resolves as the entity that already exists.
    pub fn trust_of(&self, directory_id: &str, remote_domain_name: &str) -> Option<&TrustRecord> {
        let key = domain_key(remote_domain_name);
        self.trusts.values().find(|trust| {
            trust.directory_id == directory_id && domain_key(&trust.remote_domain_name) == key
        })
    }

    /// How many trust relationships `directory_id` currently holds — the count
    /// [`MAX_TRUSTS_PER_DIRECTORY`] bounds.
    pub fn trust_count(&self, directory_id: &str) -> usize {
        self.trusts
            .values()
            .filter(|trust| trust.directory_id == directory_id)
            .count()
    }

    /// How many manual snapshots `directory_id` currently holds — the count
    /// [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`] bounds.
    pub fn manual_snapshot_count(&self, directory_id: &str) -> usize {
        self.snapshots
            .values()
            .filter(|snapshot| {
                snapshot.directory_id == directory_id
                    && snapshot.snapshot_type == SNAPSHOT_TYPE_MANUAL
            })
            .count()
    }

    /// The manual snapshot limits `GetSnapshotLimits` reports for `directory_id`, as
    /// the modelled `SnapshotLimits`.
    ///
    /// Every member is derived rather than stored: the limit is the quota
    /// [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`], the current count is what the snapshot
    /// store holds for that directory ([`Self::manual_snapshot_count`]), and
    /// `ManualSnapshotsLimitReached` ("Indicates if the manual snapshot limit has been
    /// reached") is the comparison of the two.
    pub fn snapshot_limits(&self, directory_id: &str) -> SnapshotLimits {
        let taken = self.manual_snapshot_count(directory_id);
        SnapshotLimits {
            manual_snapshots_limit: Some(clamp_to_i32(MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY)),
            manual_snapshots_current_count: Some(clamp_to_i32(taken)),
            manual_snapshots_limit_reached: taken >= MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY,
        }
    }

    /// The Region's directory limits `GetDirectoryLimits` reports, as the modelled
    /// `DirectoryLimits`.
    ///
    /// Every member is derived rather than stored. Each of the three counters the
    /// structure carries — cloud-only, cloud-only Managed Microsoft AD, connected —
    /// reports ITS OWN category's figures: [`Self::directories_in_category`] of that
    /// [`DirectoryLimitCategory`] as the current count, that category's published
    /// Region quota ([`DirectoryLimitCategory::quota`]) as the limit, and the
    /// `...LimitReached` flag ("Indicates if the … limit has been reached") as the
    /// comparison of the two — so the report says exactly when a further create of that
    /// category is refused with `DirectoryLimitExceededException`, which is what that
    /// error's own documentation points at this operation for.
    pub fn directory_limits(&self) -> DirectoryLimits {
        let cloud_only = self.category_report(DirectoryLimitCategory::CloudOnly);
        let microsoft_ad = self.category_report(DirectoryLimitCategory::CloudOnlyMicrosoftAd);
        let connected = self.category_report(DirectoryLimitCategory::Connected);
        DirectoryLimits {
            cloud_only_directories_limit: Some(cloud_only.limit),
            cloud_only_directories_current_count: Some(cloud_only.current_count),
            cloud_only_directories_limit_reached: cloud_only.limit_reached,
            cloud_only_microsoft_ad_limit: Some(microsoft_ad.limit),
            cloud_only_microsoft_ad_current_count: Some(microsoft_ad.current_count),
            cloud_only_microsoft_ad_limit_reached: microsoft_ad.limit_reached,
            connected_directories_limit: Some(connected.limit),
            connected_directories_current_count: Some(connected.current_count),
            connected_directories_limit_reached: connected.limit_reached,
        }
    }

    /// One counter of [`Self::directory_limits`]: the category's own quota, how many
    /// stored directories fall in it, and whether the quota is used up.
    fn category_report(&self, category: DirectoryLimitCategory) -> CategoryReport {
        let count = self.directories_in_category(category);
        let quota = category.quota();
        CategoryReport {
            limit: clamp_to_i32(quota),
            current_count: clamp_to_i32(count),
            limit_reached: count >= quota,
        }
    }

    /// Whether a further directory of `category` can be created: that category's own
    /// published quota against how many of it the store already holds. The one place a
    /// create decides whether it is past the quota, so the bound a create is refused at
    /// and the bound `GetDirectoryLimits` reports reached cannot drift apart.
    pub fn category_limit_reached(&self, category: DirectoryLimitCategory) -> bool {
        self.directories_in_category(category) >= category.quota()
    }
}

/// One counter of the modelled `DirectoryLimits`, as
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::category_report`]
/// derives it.
#[derive(Debug, Clone, Copy)]
struct CategoryReport {
    /// The category's published Region quota.
    limit: i32,
    /// How many stored directories fall in the category.
    current_count: i32,
    /// Whether the quota is used up.
    limit_reached: bool,
}

/// A count as the modelled `Limit` integer, which is 32 bits wide: every count a
/// `SnapshotLimits` or a `DirectoryLimits` reports is bounded by [`MAX_SNAPSHOTS`] or
/// [`MAX_DIRECTORIES`], far below the maximum, so the saturation can never be reached
/// and is here only so no cast can wrap.
fn clamp_to_i32(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// The opaque `NextToken` for a page of `kind` that ended at `value`.
///
/// Hex-encoded so the token carries no meaning a client could construct, and tagged
/// with both a version and the kind of listing that issued it, so a token replayed
/// against another operation is refused rather than misread.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{kind}:{value}"))
}

/// The cursor value inside a `kind` token, or `None` when `token` is not one this
/// service issued for that listing — which every caller answers with
/// `InvalidNextTokenException`.
///
/// The length check comes FIRST, so a caller-sized token is never decoded into a
/// caller-sized buffer: see [`MAX_NEXT_TOKEN_CHARS`].
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    if token.len() > MAX_NEXT_TOKEN_CHARS {
        return None;
    }
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let rest = decoded.strip_prefix(&format!("{CURSOR_VERSION}:{kind}:"))?;
    Some(rest.to_string())
}

/// The page size a `Limit` member resolves to.
///
/// From `Limit`'s own documentation: "The maximum number of items to return. If this
/// value is zero, the maximum number of items is specified by the limitations of the
/// operation" — so zero, and an absent member, both mean [`DEFAULT_PAGE_SIZE`].
pub fn page_size(limit: Option<i32>) -> usize {
    limit
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

/// A `SystemTime` as the modelled timestamp type.
pub fn as_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The source of every request id this service reports.
///
/// Process-wide rather than a field of the state struct on purpose: a REPORTING
/// operation reports a request id too when it refuses a call, and a read must not write
/// to the store it reports on.
static MINTED_REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// A fresh id for the call being answered, shaped the way the modelled `RequestId`
/// demands.
///
/// Every error structure in this service's model carries a `RequestId` whose `@pattern`
/// is `^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$`
/// (`smithy_query(query="shape",shape="EntityDoesNotExistException")`), so the value is
/// rendered as an RFC 4122 version-4 UUID. Its low field is a monotonic counter rather
/// than randomness, so the id is unique for the life of the process without being
/// derived from the request: two identical calls are answered with two different ids,
/// and no id is ever reused.
pub fn next_request_id() -> String {
    let minted = MINTED_REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{minted:012x}")
}

/// `message` with the request-id suffix Directory Service appends to the diagnostic of
/// every refusal: `… : RequestId: <id>`.
///
/// A recorded ds refusal carries its request id TWICE — in the modelled `RequestId`
/// member and at the end of `Message` — so the two are built together in [`refusal`] and
/// no handler can report one without the other.
fn with_request_id(message: &str, request_id: &str) -> String {
    format!("{message} : RequestId: {request_id}")
}

/// One of this service's modelled error structures.
///
/// Every error shape Directory Service models carries the same two members — `Message`
/// and the UUID-shaped `RequestId` — and the macro below implements this for each of
/// them, so [`refusal`] is the single place either member is ever filled.
pub trait ModeledFailure: Sized {
    /// The structure carrying `message` and `request_id`.
    fn from_parts(message: String, request_id: String) -> Self;
}

/// Implement [`ModeledFailure`] for each named error shape of this service's model.
macro_rules! modeled_failures {
    ($($error:ident),+ $(,)?) => {
        $(
            impl ModeledFailure for crate::ops::types::$error {
                fn from_parts(message: String, request_id: String) -> Self {
                    Self {
                        message: Some(message),
                        request_id: Some(request_id),
                    }
                }
            }
        )+
    };
}

modeled_failures!(
    ADAssessmentLimitExceededException,
    AccessDeniedException,
    AuthenticationFailedException,
    CertificateDoesNotExistException,
    ClientException,
    DirectoryAlreadySharedException,
    DirectoryDoesNotExistException,
    DirectoryInDesiredStateException,
    DirectoryLimitExceededException,
    DirectoryUnavailableException,
    EntityAlreadyExistsException,
    EntityDoesNotExistException,
    InsufficientPermissionsException,
    InvalidClientAuthStatusException,
    InvalidLDAPSStatusException,
    InvalidNextTokenException,
    InvalidParameterException,
    InvalidTargetException,
    IpRouteLimitExceededException,
    NoAvailableCertificateException,
    ServiceException,
    SnapshotLimitExceededException,
    TagLimitExceededException,
    UnsupportedOperationException,
);

/// Refuse the call with one of this service's modelled errors, `message` as its
/// diagnostic.
///
/// THE one place a ds refusal is built: it mints the call's request id
/// ([`next_request_id`]), reports it in the modelled `RequestId` member and appends it
/// to the message ([`with_request_id`]), so no refusal leaves this service without the
/// correlation handle an AWS client, an SDK retry log or a support workflow reads a
/// failed call by. WHICH error is refused is the error-enum variant the value is
/// constructed into, so a handler names it exactly where it did before.
pub fn refusal<E: ModeledFailure>(message: impl Into<String>) -> E {
    let request_id = next_request_id();
    E::from_parts(with_request_id(&message.into(), &request_id), request_id)
}

/// Refuse the call with one of this service's modelled errors, where the diagnostic
/// ITSELF ends with the call's request-id suffix and the envelope's own suffix follows
/// it.
///
/// Only the tag family's diagnostic ([`invalid_resource_id`]) is recorded that way,
/// while every other family's carries the suffix once ([`refusal`]): the inner suffix
/// belongs to the layer that produces that family's text, which has already appended the
/// id by the time the envelope appends its own. Both occurrences are the id of THIS
/// call, the one the modelled `RequestId` member reports, so the message still carries
/// exactly one correlation handle.
///
/// The recorded body is where the doubling comes from, verbatim from the wire corpus's
/// `AddTagsToResource` refusal of an absent resource id (fixture
/// `ds-tags-error-paths-fake-directory#task#1`):
///
/// ```text
/// HTTP 400; EntityDoesNotExistException: Invalid resource ID. : RequestId:
/// f4a44547-8d53-4939-9c8a-7fc6f361d17a : RequestId: f4a44547-8d53-4939-9c8a-7fc6f361d17a
/// ```
///
/// The issue report that filed this quotes the same message with ONE suffix, which is an
/// abbreviation of that body: a single-suffix message replays as a divergence on
/// `$.Message` for all three tag operations, and the doubled one replays clean. Should a
/// later recording show the single form, this function and the test row that pins it go
/// away in favour of plain [`refusal`] rather than growing a second exception.
fn refusal_restating_its_request_id<E: ModeledFailure>(diagnostic: &str) -> E {
    let request_id = next_request_id();
    let stated = with_request_id(diagnostic, &request_id);
    E::from_parts(with_request_id(&stated, &request_id), request_id)
}

/// The `EntityDoesNotExistException` message for an id that names no stored directory.
/// Shared so every operation that resolves a directory says the same thing about the
/// same failure.
///
/// The wording follows Directory Service's own: the quoted id the request asked for,
/// then "does not exist." — so the message is built from the request and names what the
/// caller has to fix, and nothing else. [`refusal`] appends the request id, once.
pub fn no_such_directory(directory_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist.")
}

/// The `EntityDoesNotExistException` the tag API answers for a `ResourceId` that names
/// no taggable resource.
///
/// `AddTagsToResource` / `ListTagsForResource` / `RemoveTagsFromResource` take a
/// `ResourceId`, not a `DirectoryId`, and the recorded refusal of a well-formed but
/// absent one is the tag family's own diagnostic: it says the id is not a resource this
/// service can tag, and QUOTES NO ID — unlike the directory-lookup family
/// ([`no_such_directory`]), which names the id it could not resolve. The two families
/// are what a caller tells "the id you passed is not a taggable resource id" from "this
/// directory is gone" by, so they must not be collapsed into one template.
///
/// Built through [`refusal_restating_its_request_id`]: the recorded refusal of all three
/// operations carries the call's id in the message twice.
pub fn invalid_resource_id<E: ModeledFailure>() -> E {
    refusal_restating_its_request_id(INVALID_RESOURCE_ID)
}

/// The refusal of a `NextToken` this service did not issue, or issued for another
/// listing.
///
/// The diagnostic is `InvalidNextTokenException`'s own documented sentence
/// (`smithy_query(query="shape",shape="InvalidNextTokenException")`: "The `NextToken`
/// value is not valid."), with the family-wide request-id suffix [`refusal`] appends.
/// The recorded text of this particular refusal is withheld by the corpus, so the
/// sentence stated here is the model's own rather than a recorded one. The error SHAPE
/// is the variant the caller constructs this into, because not every paginated operation
/// of this service models `InvalidNextTokenException` for it.
pub fn invalid_next_token<E: ModeledFailure>() -> E {
    refusal(INVALID_NEXT_TOKEN)
}

/// The tag family's diagnostic, without either request-id suffix: see
/// [`invalid_resource_id`], which is what builds the refusal.
const INVALID_RESOURCE_ID: &str = "Invalid resource ID.";

/// The ARN of the Amazon SNS topic named `topic_name` in `region`.
///
/// Built through [`::engine::emulator_arn`], so the account segment is the one
/// canonical account id every generated emulator presents. An SNS topic ARN's resource
/// portion is the bare topic name.
pub fn sns_topic_arn(region: &str, topic_name: &str) -> String {
    ::engine::emulator_arn(SNS_ARN_SERVICE, region, topic_name)
}

/// The Region a reported ARN carries, and the Region an operation that acts per Region
/// (`RemoveRegion`) acts on: the Region of the caller's sigv4 credential scope, or
/// [`DEFAULT_REGION`] when the request carries none.
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single source
/// of SigV4-scope parsing, so a header that is not a well-formed sigv4 `Authorization`
/// yields the default rather than a bogus Region.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `EntityDoesNotExistException` message for an id that names no stored snapshot.
/// Shared so every operation that resolves a snapshot says the same thing about the
/// same failure.
pub fn no_such_snapshot(snapshot_id: &str) -> String {
    format!("Snapshot {snapshot_id} does not exist")
}

/// The `EntityDoesNotExistException` message for an id that names no stored trust
/// relationship. Shared so every operation that resolves a trust says the same thing
/// about the same failure.
///
/// It quotes the id the request asked for and says it does not exist, the way the
/// directory-lookup family's wording does ([`no_such_directory`]), so the message is built
/// from the request and names what the caller has to fix. [`refusal`] appends the request
/// id.
pub fn no_such_trust(trust_id: &str) -> String {
    format!("Trust: '{trust_id}' does not exist.")
}

/// The `EntityDoesNotExistException` message for an id that names no stored assessment.
///
/// It names the member and the id the request sent, the way Directory Service's own
/// message for this failure does, so the caller can tell which id was rejected.
/// [`refusal`] appends the request id.
pub fn no_such_assessment(assessment_id: &str) -> String {
    format!("Assessment Id {assessment_id} does not exist.")
}

/// The `InvalidNextTokenException` diagnostic for a token this service did not issue:
/// the sentence the error shape's own documentation states, which [`invalid_next_token`]
/// is what refuses the call with.
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid.";

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the Directory Service emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDirectoryserviceDirectoryservice20150416State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDirectoryserviceDirectoryservice20150416State>)
}
