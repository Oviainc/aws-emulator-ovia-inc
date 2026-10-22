// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-emr-elasticmapreduce service.
//!
//! The job-flow (cluster) lifecycle lives here: `RunJobFlow` inserts a
//! [`ClusterRecord`] built from the request, every later request resolves that record
//! out of the single shared engine, and the EMR wire shapes
//! (`Cluster`/`ClusterSummary`/`Step`/`StepSummary`) are PROJECTIONS of it — see
//! [`ClusterRecord::to_cluster`], [`ClusterRecord::to_summary`],
//! [`StepRecord::to_step`] and [`StepRecord::to_summary`]. No handler builds one of
//! those shapes from its own input.
//!
//! Beside the clusters sits the account's single block public access configuration
//! ([`BlockPublicAccessRecord`]), which always exists: it starts as the documented
//! account default, `PutBlockPublicAccessConfiguration` replaces it whole, and
//! `GetBlockPublicAccessConfiguration` reports it.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Application, BlockPublicAccessConfiguration, BlockPublicAccessConfigurationMetadata,
    BootstrapActionConfig, Cluster, ClusterStateChangeReason, ClusterStatus, ClusterSummary,
    ClusterTimeline, Configuration, Ec2InstanceAttributes, HadoopStepConfig, InstanceFleetConfig,
    JobFlowInstancesConfig, KerberosAttributes, ManagedScalingPolicy, MonitoringConfiguration,
    NotebookExecution, OSRelease, PlacementGroupConfig, PortRange, Session, SessionMappingDetail,
    SimplifiedApplication, Step, StepConfig, StepStatus, StepSummary, StepTimeline, Studio,
    SupportedInstanceType, Tag,
};

/// `com.amazonaws.emr#ClusterState`, as the wire carries it.
pub mod cluster_state {
    pub const STARTING: &str = "STARTING";
    pub const BOOTSTRAPPING: &str = "BOOTSTRAPPING";
    pub const RUNNING: &str = "RUNNING";
    pub const WAITING: &str = "WAITING";
    pub const TERMINATING: &str = "TERMINATING";
    pub const TERMINATED: &str = "TERMINATED";
    pub const TERMINATED_WITH_ERRORS: &str = "TERMINATED_WITH_ERRORS";

    /// The states `AddJobFlowSteps` accepts a step in: "You can only add steps to a
    /// cluster that is in one of the following states: STARTING, BOOTSTRAPPING,
    /// RUNNING, or WAITING." (`AddJobFlowSteps` doc, Smithy model.)
    pub const ACCEPTS_STEPS: &[&str] = &[STARTING, BOOTSTRAPPING, RUNNING, WAITING];
}

/// `com.amazonaws.emr#StepState`, as the wire carries it.
pub mod step_state {
    pub const PENDING: &str = "PENDING";
    pub const CANCEL_PENDING: &str = "CANCEL_PENDING";
    pub const RUNNING: &str = "RUNNING";
    pub const COMPLETED: &str = "COMPLETED";
    pub const CANCELLED: &str = "CANCELLED";
    pub const FAILED: &str = "FAILED";
    pub const INTERRUPTED: &str = "INTERRUPTED";
}

/// `com.amazonaws.emr#InstanceCollectionType`.
pub mod instance_collection_type {
    pub const INSTANCE_FLEET: &str = "INSTANCE_FLEET";
    pub const INSTANCE_GROUP: &str = "INSTANCE_GROUP";
}

/// `com.amazonaws.emr#InstanceFleetState`, as the wire carries it.
pub mod instance_fleet_state {
    pub const PROVISIONING: &str = "PROVISIONING";
    pub const RUNNING: &str = "RUNNING";
}

/// `com.amazonaws.emr#CancelStepsRequestStatus`: "The value may be SUBMITTED or
/// FAILED." (`CancelStepsInfo$Status` doc.)
pub mod cancel_steps_request_status {
    pub const SUBMITTED: &str = "SUBMITTED";
    pub const FAILED: &str = "FAILED";
}

/// `com.amazonaws.emr#NotebookExecutionStatus`, for the statuses this emulator sets.
pub mod notebook_execution_status {
    /// "`STOP_PENDING` indicates that the cluster has received a
    /// `StopNotebookExecution` request and the stop is pending."
    pub const STOP_PENDING: &str = "STOP_PENDING";
    /// "`STOPPED` indicates that the execution stopped because of a
    /// `StopNotebookExecution` request."
    pub const STOPPED: &str = "STOPPED";
    /// "`FINISHED` indicates that the execution has completed without error."
    pub const FINISHED: &str = "FINISHED";
    /// "`FAILED` indicates that the execution failed."
    pub const FAILED: &str = "FAILED";

    /// The statuses a notebook execution never leaves, so a stop has nothing left to
    /// do (`NotebookExecutionStatus` doc).
    pub const TERMINAL: &[&str] = &[FINISHED, FAILED, STOPPED];
}

/// `com.amazonaws.emr#AuthMode`
/// (`smithy_query(query="shape", shape="AuthMode")`: SSO, IAM).
pub mod auth_mode {
    pub const IAM: &str = "IAM";
}

/// `com.amazonaws.emr#ClusterStateChangeReasonCode`, for the codes this emulator sets.
pub mod cluster_state_change_reason_code {
    pub const USER_REQUEST: &str = "USER_REQUEST";
}

/// How a cluster-scoped operation that models no client error refuses a `ClusterId` the
/// service holds no cluster for.
///
/// AWS does not document the wording, so the text is built from the request: it names
/// the id the caller asked for. What IS the contract is the refusal itself — HTTP 400
/// with the error code `ValidationException` — rather than an empty success.
pub fn unknown_cluster_message(cluster_id: &str) -> String {
    format!("Cluster id '{cluster_id}' is not valid.")
}

/// Refuse a `ManagedScalingPolicy` whose compute limits cannot hold together.
///
/// `MinimumCapacityUnits` is "the lower boundary of Amazon EC2 units" and
/// `MaximumCapacityUnits` "the upper boundary of Amazon EC2 units"
/// (`ComputeLimits` member docs, Smithy model), so a minimum above the maximum names no
/// range the scaling activity could stay inside. Answers the refusal text, which names
/// the members and the values the request carried.
pub fn validate_managed_scaling_policy(policy: &ManagedScalingPolicy) -> Result<(), String> {
    if let Some(limits) = policy.compute_limits.as_ref() {
        if limits.minimum_capacity_units > limits.maximum_capacity_units {
            return Err(format!(
                "ComputeLimits.MinimumCapacityUnits {} is above \
                 ComputeLimits.MaximumCapacityUnits {}",
                limits.minimum_capacity_units, limits.maximum_capacity_units
            ));
        }
    }
    Ok(())
}

/// The region an ARN carries when the caller's request names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a clock-owed cluster transition
/// (`STARTING` -> ready, `TERMINATING` -> `TERMINATED`) comes due.
///
/// CHOSEN, not documented: the real service takes minutes. It is deliberately
/// non-zero so a caller acting straight after `RunJobFlow`/`TerminateJobFlows` still
/// observes the transient state, and it is stored as a due instant — nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `ListClusters` page size: "This call returns a maximum of 50 clusters in unsorted
/// order per call, but returns a marker to track the paging of the cluster list across
/// multiple ListClusters calls." (`ListClusters` doc, Smithy model.)
pub const LIST_CLUSTERS_PAGE_SIZE: usize = 50;

/// `ListSteps` page size: "The CLI automatically paginates results to return a list
/// greater than 50 steps. To return more than 50 steps using the CLI, specify a
/// `Marker` …" (`ListSteps` doc, Smithy model.)
pub const LIST_STEPS_PAGE_SIZE: usize = 50;

/// "You can specify a maximum of 10 `stepIDs`." (`ListSteps` doc, Smithy model.)
pub const LIST_STEPS_MAX_STEP_IDS: usize = 10;

/// "A maximum of 256 steps are allowed in each job flow." (`RunJobFlow` and
/// `AddJobFlowSteps` docs, Smithy model.)
pub const MAX_STEPS_PER_JOB_FLOW: usize = 256;

/// "The maximum number of clusters allowed is 10." (`TerminateJobFlows` doc, Smithy
/// model.)
pub const MAX_TERMINATE_JOB_FLOW_IDS: usize = 10;

/// "Specifies the number of steps that can be executed concurrently. The default value
/// is 1. The maximum value is 256." (`RunJobFlow$StepConcurrencyLevel` doc, Smithy
/// model.)
pub const DEFAULT_STEP_CONCURRENCY_LEVEL: i32 = 1;
/// See [`DEFAULT_STEP_CONCURRENCY_LEVEL`].
pub const MAX_STEP_CONCURRENCY_LEVEL: i32 = 256;

/// "A maximum of 256 steps are allowed in each CancelSteps request." (`CancelSteps`
/// doc, Smithy model.)
pub const MAX_CANCEL_STEP_IDS: usize = 256;

/// The step states a cancel can still act on: "When you use Amazon EMR releases 5.28.0
/// and later, you can cancel steps that are in a `PENDING` or `RUNNING` state."
/// (`CancelSteps` doc, Smithy model.)
pub const CANCELLABLE_STEP_STATES: &[&str] = &[step_state::PENDING, step_state::RUNNING];

/// "A Studio can have a maximum of 5 subnets." (`CreateStudio$SubnetIds` doc, Smithy
/// model.)
pub const MAX_STUDIO_SUBNETS: usize = 5;

/// How many EMR Studios this emulator holds at once.
///
/// CHOSEN, not documented: AWS bounds Studios per account, and this number is what
/// stops the studio map growing without end — a create past it is refused with the
/// modelled `InvalidRequestException` rather than accepted.
pub const MAX_STUDIOS: usize = 100;

/// How many session mappings one Studio holds at once.
///
/// CHOSEN, not documented, for the same reason [`MAX_STUDIOS`] is: the identity a
/// mapping is keyed by comes from the request, so without a bound one studio could
/// accumulate mappings without end.
pub const MAX_SESSION_MAPPINGS_PER_STUDIO: usize = 500;

/// How many instance fleets one cluster holds at once.
///
/// CHOSEN, not documented here, and shaped after EMR's instance-group limit (a cluster
/// carries one master and one core group plus up to 48 task groups): it is what stops
/// one cluster's fleet list growing without end, the same way
/// [`MAX_STEPS_PER_JOB_FLOW`] bounds its step list.
pub const MAX_INSTANCE_FLEETS_PER_CLUSTER: usize = 50;

/// The account-wide default for `BlockPublicSecurityGroupRules`: "By default, the value
/// is `false` for accounts that have created Amazon EMR clusters before July 2019. For
/// accounts created after this, the default is `true`."
/// (`BlockPublicAccessConfiguration$BlockPublicSecurityGroupRules` doc, Smithy model.)
/// This emulator's account created no cluster before July 2019, so it takes the later
/// default.
pub const DEFAULT_BLOCK_PUBLIC_SECURITY_GROUP_RULES: bool = true;

/// The port the account-wide default permits publicly: "By default, Port 22, which is
/// used for SSH access to the cluster Amazon EC2 instances, is in the list of
/// `PermittedPublicSecurityGroupRuleRanges`."
/// (`BlockPublicAccessConfiguration$PermittedPublicSecurityGroupRuleRanges` doc, Smithy
/// model.)
pub const DEFAULT_PERMITTED_PUBLIC_PORT: i32 = 22;

/// `com.amazonaws.emr#Port`'s published bounds, which `PortRange$MinRange` and
/// `PortRange$MaxRange` both target: `range { min: -1, max: 65535 }` (Smithy model).
pub const PORT_MIN: i32 = -1;
/// See [`PORT_MIN`].
pub const PORT_MAX: i32 = 65535;

/// How many ranges an exception list may carry, and the cap on everything one
/// `PutBlockPublicAccessConfiguration` stores.
///
/// DERIVED, not documented: a permitted range covers at least one of the
/// [`PORT_MIN`]..=[`PORT_MAX`] ports and the ranges may not overlap, so a list longer
/// than the port space necessarily repeats a port and can be refused by its length
/// alone — before any per-element work is done. The same number caps the entries of a
/// stored configuration as a whole (ranges, nested configurations and properties
/// entries), so one `Put` cannot make every later `GetBlockPublicAccessConfiguration`
/// clone and serialise an arbitrarily large body.
pub const MAX_PERMITTED_PUBLIC_RANGES: usize = (PORT_MAX - PORT_MIN + 1) as usize;
/// See [`MAX_PERMITTED_PUBLIC_RANGES`].
pub const MAX_CONFIGURATION_ENTRIES: usize = MAX_PERMITTED_PUBLIC_RANGES;

/// Width of the uppercase base-36 body of a minted `j-`/`s-` id.
const ID_BODY_WIDTH: usize = 13;

/// The largest `CreateSecurityConfiguration$SecurityConfiguration` document this
/// emulator accepts, in bytes.
///
/// CHOSEN, not documented: the model leaves that member an unconstrained `String`, but
/// an unbounded document would be parsed and then retained for the life of the process.
/// 256 KiB is two orders of magnitude above the longest bounded string EMR models
/// (`XmlString`, 10280 bytes) and far above any real security configuration, so a
/// faithful request is never refused by it. A document past it is refused BEFORE it is
/// parsed.
pub const MAX_SECURITY_CONFIGURATION_BYTES: usize = 256 * 1024;

/// How many security configurations this emulator stores for the account.
///
/// CHOSEN, not documented: EMR publishes a service quota on security configurations per
/// account and region but the model carries no value for it, so this stands in for it —
/// high enough that no faithful client reaches it, and a bound rather than unlimited
/// growth. Reaching it is refused with the operation's modeled client error, the way a
/// quota is.
pub const MAX_SECURITY_CONFIGURATIONS: usize = 1_000;

/// Per-service state for com-amazonaws-emr-elasticmapreduce. A handler reaches it through [`service_state`].
///
/// `clusters` is keyed by cluster id, and a minted id is the zero-padded base-36
/// rendering of a monotonic sequence, so the map's key order IS creation order and
/// every listing is deterministic.
#[derive(Debug)]
pub struct ComAmazonawsEmrElasticmapreduceState {
    pub clusters: BTreeMap<String, ClusterRecord>,
    pub next_cluster_seq: u64,
    /// One counter for every step of every cluster, so a step id is unique
    /// service-wide rather than only within its own cluster.
    pub next_step_seq: u64,
    /// One counter for every instance fleet of every cluster, for the same reason.
    pub next_instance_fleet_seq: u64,
    /// The persisted EMR Studios, keyed by studio id. `CreateStudio` inserts the
    /// `Studio` the request describes plus the id, ARN, URL and creation instant it
    /// mints, and every studio operation resolves that record: the shape is complete
    /// and nothing is derived from it, so it is held as the wire shape rather than
    /// projected (unlike [`ClusterRecord`], whose wire shapes ARE projections).
    /// Bounded by [`MAX_STUDIOS`].
    pub studios: BTreeMap<String, Studio>,
    pub next_studio_seq: u64,
    /// The persisted studio session mappings, keyed by
    /// [`session_mapping_key`] — one mapping per (studio, identity type, identity),
    /// which is the identity `GetStudioSessionMapping` addresses one by. Bounded per
    /// studio by [`MAX_SESSION_MAPPINGS_PER_STUDIO`].
    pub studio_session_mappings: BTreeMap<String, SessionMappingDetail>,
    /// The persisted notebook executions, keyed by execution id.
    ///
    /// `StartNotebookExecution` is what inserts one; it is still a scaffold stub (its
    /// own issue), so until it lands this map is what tells `DescribeNotebookExecution`
    /// and `StopNotebookExecution` that an execution id names nothing.
    pub notebook_executions: BTreeMap<String, NotebookExecution>,
    /// The ONE block public access configuration of this account and Region, which
    /// always exists: `PutBlockPublicAccessConfiguration` REPLACES it and
    /// `GetBlockPublicAccessConfiguration` reports it.
    pub block_public_access: BlockPublicAccessRecord,
    /// Security configurations keyed by NAME: a security configuration "is stored in
    /// the service and can be specified when a cluster is created"
    /// (`CreateSecurityConfiguration` doc, Smithy model) by that name alone, so the
    /// name is its identity. Keying the map by it is what makes the name unique and
    /// what gives every listing a deterministic, name-ordered sequence.
    pub security_configurations: BTreeMap<String, SecurityConfigurationRecord>,
}

/// The account's default configuration is in place before any caller puts one, so the
/// state starts out holding it rather than an absent one — exactly what
/// `GetBlockPublicAccessConfiguration` reports for an account that has never called
/// `PutBlockPublicAccessConfiguration`. Its `CreationDateTime` is fixed when this store
/// is first reached and never moves again, so repeated reads report the same instant.
impl Default for ComAmazonawsEmrElasticmapreduceState {
    fn default() -> Self {
        Self {
            clusters: BTreeMap::new(),
            next_cluster_seq: 0,
            next_step_seq: 0,
            next_instance_fleet_seq: 0,
            studios: BTreeMap::new(),
            next_studio_seq: 0,
            studio_session_mappings: BTreeMap::new(),
            notebook_executions: BTreeMap::new(),
            block_public_access: BlockPublicAccessRecord::account_default(SystemTime::now()),
            security_configurations: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// A fresh cluster id: `j-` followed by uppercase alphanumerics.
    pub fn mint_cluster_id(&mut self) -> String {
        self.next_cluster_seq += 1;
        format!("j-{}", base36_id(self.next_cluster_seq))
    }

    /// A fresh step id: `s-` followed by uppercase alphanumerics.
    pub fn mint_step_id(&mut self) -> String {
        self.next_step_seq += 1;
        format!("s-{}", base36_id(self.next_step_seq))
    }

    /// A fresh instance fleet id: `if-` followed by uppercase alphanumerics.
    pub fn mint_instance_fleet_id(&mut self) -> String {
        self.next_instance_fleet_seq += 1;
        format!("if-{}", base36_id(self.next_instance_fleet_seq))
    }

    /// A fresh studio id: `es-` followed by uppercase alphanumerics.
    pub fn mint_studio_id(&mut self) -> String {
        self.next_studio_seq += 1;
        format!("es-{}", base36_id(self.next_studio_seq))
    }

    /// How many session mappings the studio `studio_id` holds — the count
    /// [`MAX_SESSION_MAPPINGS_PER_STUDIO`] is taken against.
    pub fn session_mappings_of(&self, studio_id: &str) -> usize {
        let prefix = session_mapping_key_prefix(studio_id);
        self.studio_session_mappings
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .count()
    }

    /// The cluster `id`, with every transition the clock already made due applied.
    pub fn cluster_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut ClusterRecord> {
        let record = self.clusters.get_mut(id)?;
        record.advance_to(now);
        Some(record)
    }

    /// Apply the clock to every cluster. Called by the listings, which filter on state.
    pub fn advance_all(&mut self, now: SystemTime) {
        for record in self.clusters.values_mut() {
            record.advance_to(now);
        }
    }
}

/// `seq` as `ID_BODY_WIDTH` uppercase base-36 digits, zero padded.
///
/// Fixed width and padded so the lexicographic order of the ids matches the order the
/// sequence minted them in — which is what makes the `BTreeMap` creation-ordered.
fn base36_id(seq: u64) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut body = [b'0'; ID_BODY_WIDTH];
    let mut remaining = seq;
    let mut at = ID_BODY_WIDTH;
    while remaining > 0 && at > 0 {
        at -= 1;
        body[at] = DIGITS[(remaining % 36) as usize];
        remaining /= 36;
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// The ARN of a cluster: `arn:aws:elasticmapreduce:<region>:<account>:cluster/<id>`.
pub fn cluster_arn(region: &str, cluster_id: &str) -> String {
    ::engine::emulator_arn(
        "elasticmapreduce",
        region,
        &format!("cluster/{cluster_id}"),
    )
}

/// The ARN of a studio: `arn:aws:elasticmapreduce:<region>:<account>:studio/<id>`.
pub fn studio_arn(region: &str, studio_id: &str) -> String {
    ::engine::emulator_arn("elasticmapreduce", region, &format!("studio/{studio_id}"))
}

/// "The unique access URL of the Amazon EMR Studio" (`Studio$Url` doc): the studio's
/// own id under the regional EMR Studio domain, so the URL is derived from the studio
/// rather than invented per read.
pub fn studio_url(region: &str, studio_id: &str) -> String {
    format!("https://{studio_id}.emrstudio-prod.{region}.amazonaws.com")
}

/// The key one studio session mapping is held under: its studio, its identity type and
/// the identity itself.
///
/// `GetStudioSessionMapping` addresses a mapping by `StudioId` + `IdentityType` +
/// either `IdentityId` or `IdentityName` ("Specify `IdentityId` or `IdentityName`"),
/// so those three are its identity. The identity is case-folded because an IAM
/// Identity Center user or group name is not distinguished by case.
pub fn session_mapping_key(studio_id: &str, identity_type: &str, identity: &str) -> String {
    format!(
        "{}{identity_type}\u{1f}{}",
        session_mapping_key_prefix(studio_id),
        identity.to_lowercase()
    )
}

/// The identity a studio session mapping names, and which member named it.
///
/// "Either `IdentityName` or `IdentityId` must be specified, but not both."
/// (`CreateStudioSessionMapping$IdentityId` doc.) `None` means the request named
/// neither or both, which every mapping operation refuses.
pub fn session_mapping_identity(
    identity_id: Option<&String>,
    identity_name: Option<&String>,
) -> Option<(Option<String>, Option<String>, String)> {
    let identity_id = identity_id.filter(|value| !value.trim().is_empty());
    let identity_name = identity_name.filter(|value| !value.trim().is_empty());
    match (identity_id, identity_name) {
        (Some(id), None) => Some((Some(id.clone()), None, id.clone())),
        (None, Some(name)) => Some((None, Some(name.clone()), name.clone())),
        _ => None,
    }
}

/// The prefix every session-mapping key of `studio_id` begins with, so the mappings
/// that belong to one studio can be found (and dropped with it) without a second map
/// keyed by studio.
pub fn session_mapping_key_prefix(studio_id: &str) -> String {
    format!("{studio_id}\u{1f}")
}

/// The region this request is addressed to: the region of its SigV4 credential scope,
/// or [`DEFAULT_REGION`] when the request carries none.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The account's block public access configuration, with the metadata EMR keeps beside
/// it: "Properties that describe the Amazon Web Services principal that created the
/// `BlockPublicAccessConfiguration` using the `PutBlockPublicAccessConfiguration` action
/// as well as the date and time that the configuration was created. Each time a
/// configuration for block public access is updated, Amazon EMR updates this metadata."
/// (`BlockPublicAccessConfigurationMetadata` doc, Smithy model.)
///
/// The configuration is held as the wire shape the caller supplied, VERBATIM: a `Put`
/// replaces the whole configuration, so what a later `Get` reports is what was put —
/// an absent exception list stays absent, an empty one stays empty, and every port
/// bound keeps the value it was given.
#[derive(Debug, Clone)]
pub struct BlockPublicAccessRecord {
    pub configuration: BlockPublicAccessConfiguration,
    pub creation_date_time: SystemTime,
    pub created_by_arn: String,
}

impl BlockPublicAccessRecord {
    /// The configuration an account has before anybody puts one: block public access on,
    /// with Port 22 permitted. See [`DEFAULT_BLOCK_PUBLIC_SECURITY_GROUP_RULES`] and
    /// [`DEFAULT_PERMITTED_PUBLIC_PORT`].
    ///
    /// No caller created it, so the principal it names is the account itself rather
    /// than an invented user.
    pub fn account_default(at: SystemTime) -> Self {
        Self {
            configuration: BlockPublicAccessConfiguration {
                block_public_security_group_rules: DEFAULT_BLOCK_PUBLIC_SECURITY_GROUP_RULES,
                permitted_public_security_group_rule_ranges: Some(vec![PortRange {
                    min_range: DEFAULT_PERMITTED_PUBLIC_PORT,
                    max_range: Some(DEFAULT_PERMITTED_PUBLIC_PORT),
                }]),
                classification: None,
                configurations: None,
                properties: None,
            },
            creation_date_time: at,
            created_by_arn: ::engine::emulator_arn("iam", "", "root"),
        }
    }

    /// What `PutBlockPublicAccessConfiguration` stored, under the principal that called
    /// it.
    pub fn put(
        configuration: BlockPublicAccessConfiguration,
        created_by_arn: String,
        at: SystemTime,
    ) -> Self {
        Self {
            configuration,
            creation_date_time: at,
            created_by_arn,
        }
    }

    /// The wire `BlockPublicAccessConfigurationMetadata` for this record.
    pub fn to_metadata(&self) -> BlockPublicAccessConfigurationMetadata {
        BlockPublicAccessConfigurationMetadata {
            creation_date_time: timestamp(self.creation_date_time),
            created_by_arn: self.created_by_arn.clone(),
        }
    }
}

/// The IAM principal this request was signed by: what a configuration it writes records
/// as having created it.
///
/// The emulator models no identity store beyond the access key id a request is signed
/// with, which is exactly what names the caller here; it is reduced to the characters an
/// access key id can hold, and an unsigned request is the account's `default` user.
pub fn caller_arn(parts: &::http::request::Parts) -> String {
    let filtered: String = ::engine::extract_akid(&parts.headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let user = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{user}"))
}

// ---------------------------------------------------------------------------
// The Amazon EMR release catalog
// ---------------------------------------------------------------------------
//
// `DescribeReleaseLabel` reports "the releases available [in] the Region where the API
// request is run, and the available applications for a specific Amazon EMR release
// label" (its own doc), so the catalog describes the SERVICE rather than anything a
// caller created: it is a table here, beside the per-caller state, and reading it
// changes nothing a later caller sees.
//
// Source for the labels, the application versions and which release first (or last)
// bundles an application: the Amazon EMR Release Guide's release-version and
// "Application versions" tables. Application names are spelled as
// `SimplifiedApplication$Name` documents them ("For example, `hadoop`") and as
// `ReleaseLabelFilter$Application` filters on them ("For example, `spark@2.1.0`").

/// One Amazon EMR release line (`emr-<major>.<minor>`) and the patch releases published
/// on it. The labels the catalog offers are exactly these `(line, patch)` pairs.
#[derive(Debug, Clone, Copy)]
pub struct ReleaseLine {
    pub major: u32,
    pub minor: u32,
    pub patches: &'static [u32],
}

/// Every release line this emulator offers, oldest first.
pub const RELEASE_LINES: &[ReleaseLine] = &[
    ReleaseLine { major: 5, minor: 30, patches: &[0, 1, 2] },
    ReleaseLine { major: 5, minor: 31, patches: &[0] },
    ReleaseLine { major: 5, minor: 32, patches: &[0] },
    ReleaseLine { major: 5, minor: 33, patches: &[0, 1] },
    ReleaseLine { major: 5, minor: 34, patches: &[0] },
    ReleaseLine { major: 5, minor: 35, patches: &[0] },
    ReleaseLine { major: 5, minor: 36, patches: &[0, 1, 2] },
    ReleaseLine { major: 6, minor: 0, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 1, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 2, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 3, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 4, patches: &[0] },
    ReleaseLine { major: 6, minor: 5, patches: &[0] },
    ReleaseLine { major: 6, minor: 6, patches: &[0] },
    ReleaseLine { major: 6, minor: 7, patches: &[0] },
    ReleaseLine { major: 6, minor: 8, patches: &[0] },
    ReleaseLine { major: 6, minor: 9, patches: &[0] },
    ReleaseLine { major: 6, minor: 10, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 11, patches: &[0, 1] },
    ReleaseLine { major: 6, minor: 12, patches: &[0] },
    ReleaseLine { major: 6, minor: 13, patches: &[0] },
    ReleaseLine { major: 6, minor: 14, patches: &[0] },
    ReleaseLine { major: 6, minor: 15, patches: &[0] },
    ReleaseLine { major: 7, minor: 0, patches: &[0] },
    ReleaseLine { major: 7, minor: 1, patches: &[0] },
    ReleaseLine { major: 7, minor: 2, patches: &[0] },
    ReleaseLine { major: 7, minor: 3, patches: &[0] },
    ReleaseLine { major: 7, minor: 4, patches: &[0] },
    ReleaseLine { major: 7, minor: 5, patches: &[0] },
];

/// One application in the catalog.
#[derive(Debug, Clone, Copy)]
pub struct CatalogApplication {
    /// `SimplifiedApplication$Name`, as the service spells it.
    pub name: &'static str,
    /// The version this application carries from a release line onward: a row applies
    /// to every release at or after its line, up to the next row's line. Rows are in
    /// ascending line order, and a release BEFORE the first row predates the
    /// application, which such a release does not bundle at all.
    pub versions: &'static [((u32, u32), &'static str)],
    /// The line from which Amazon EMR no longer bundles this application, when it
    /// dropped it.
    pub removed_from: Option<(u32, u32)>,
}

impl CatalogApplication {
    /// The version this application carries on the release line `(major, minor)`, or
    /// `None` when that release does not bundle it.
    pub fn version_at(&self, major: u32, minor: u32) -> Option<&'static str> {
        if self.removed_from.is_some_and(|from| (major, minor) >= from) {
            return None;
        }
        self.versions
            .iter()
            .rfind(|(line, _)| *line <= (major, minor))
            .map(|&(_, version)| version)
    }
}

/// The applications Amazon EMR bundles, with the version each release line carries.
pub const APPLICATION_CATALOG: &[CatalogApplication] = &[
    CatalogApplication {
        name: "flink",
        versions: &[
            ((5, 30), "1.10.0"),
            ((5, 33), "1.12.1"),
            ((5, 36), "1.14.2"),
            ((6, 0), "1.9.1"),
            ((6, 1), "1.11.2"),
            ((6, 6), "1.14.2"),
            ((6, 9), "1.15.2"),
            ((6, 12), "1.17.0"),
            ((6, 15), "1.17.1"),
            ((7, 0), "1.18.0"),
            ((7, 2), "1.19.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "ganglia",
        versions: &[((5, 30), "3.7.2"), ((6, 0), "3.7.2")],
        // Amazon EMR 7.0.0 no longer bundles Ganglia.
        removed_from: Some((7, 0)),
    },
    CatalogApplication {
        name: "hadoop",
        versions: &[
            ((5, 30), "2.8.5"),
            ((5, 31), "2.10.0"),
            ((5, 33), "2.10.1"),
            ((6, 0), "3.2.1"),
            ((6, 9), "3.3.3"),
            ((6, 15), "3.3.6"),
            ((7, 5), "3.4.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "hbase",
        versions: &[
            ((5, 30), "1.4.13"),
            ((6, 0), "2.2.3"),
            ((6, 6), "2.4.4"),
            ((6, 9), "2.4.13"),
            ((6, 15), "2.4.17"),
            ((7, 2), "2.5.8"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "hive",
        versions: &[
            ((5, 30), "2.3.6"),
            ((5, 33), "2.3.7"),
            ((5, 36), "2.3.9"),
            ((6, 0), "3.1.2"),
            ((6, 9), "3.1.3"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "hudi",
        versions: &[
            ((5, 30), "0.5.2"),
            ((5, 36), "0.10.0"),
            ((6, 0), "0.5.0"),
            ((6, 6), "0.10.1"),
            ((6, 9), "0.12.1"),
            ((6, 12), "0.13.1"),
            ((6, 15), "0.14.0"),
            ((7, 2), "0.15.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "hue",
        versions: &[
            ((5, 30), "4.6.0"),
            ((5, 36), "4.10.0"),
            ((6, 0), "4.4.0"),
            ((6, 6), "4.10.0"),
            ((6, 9), "4.11.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "iceberg",
        // Amazon EMR offers Iceberg from 6.5.0 onward.
        versions: &[
            ((6, 5), "0.11.0"),
            ((6, 9), "0.14.1"),
            ((6, 12), "1.2.1"),
            ((6, 15), "1.4.0"),
            ((7, 0), "1.4.2"),
            ((7, 2), "1.5.0"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "jupyterenterprisegateway",
        versions: &[((6, 0), "2.1.0"), ((6, 9), "2.6.0")],
        removed_from: None,
    },
    CatalogApplication {
        name: "jupyterhub",
        versions: &[((5, 30), "1.1.0"), ((5, 36), "1.4.1"), ((6, 0), "1.1.0"), ((6, 6), "1.4.1")],
        removed_from: None,
    },
    CatalogApplication {
        name: "livy",
        versions: &[((5, 30), "0.7.0"), ((5, 36), "0.7.1"), ((6, 0), "0.7.0"), ((6, 6), "0.7.1")],
        removed_from: None,
    },
    CatalogApplication {
        name: "oozie",
        versions: &[
            ((5, 30), "5.2.0"),
            ((5, 36), "5.2.1"),
            ((6, 0), "5.1.0"),
            ((6, 1), "5.2.0"),
            ((6, 6), "5.2.1"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "phoenix",
        versions: &[
            ((5, 30), "4.14.3"),
            ((6, 0), "5.0.0"),
            ((6, 2), "5.1.2"),
            ((7, 0), "5.1.3"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "pig",
        versions: &[((5, 30), "0.17.0"), ((6, 0), "0.17.0")],
        removed_from: None,
    },
    CatalogApplication {
        name: "presto",
        versions: &[
            ((5, 30), "0.232"),
            ((5, 36), "0.267"),
            ((6, 0), "0.230"),
            ((6, 6), "0.267"),
            ((6, 9), "0.276"),
            ((6, 15), "0.283"),
            ((7, 0), "0.284"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "spark",
        versions: &[
            ((5, 30), "2.4.5"),
            ((5, 33), "2.4.7"),
            ((5, 36), "2.4.8"),
            ((6, 0), "2.4.4"),
            ((6, 1), "3.0.0"),
            ((6, 3), "3.1.1"),
            ((6, 6), "3.2.0"),
            ((6, 8), "3.3.0"),
            ((6, 12), "3.4.0"),
            ((6, 15), "3.4.1"),
            ((7, 0), "3.5.0"),
            ((7, 2), "3.5.1"),
            ((7, 5), "3.5.2"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "tez",
        versions: &[((5, 30), "0.9.2"), ((6, 0), "0.9.2"), ((6, 9), "0.10.2")],
        removed_from: None,
    },
    CatalogApplication {
        name: "trino",
        // Amazon EMR offers Trino under that name from 6.4.0 onward.
        versions: &[
            ((6, 4), "359"),
            ((6, 6), "367"),
            ((6, 9), "398"),
            ((6, 12), "414"),
            ((6, 15), "426"),
            ((7, 2), "435"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "zeppelin",
        versions: &[
            ((5, 30), "0.8.2"),
            ((5, 36), "0.10.0"),
            ((6, 0), "0.9.0"),
            ((6, 9), "0.10.1"),
            ((7, 2), "0.11.1"),
        ],
        removed_from: None,
    },
    CatalogApplication {
        name: "zookeeper",
        versions: &[((5, 30), "3.4.14"), ((6, 0), "3.4.14"), ((6, 6), "3.5.10"), ((7, 0), "3.9.1")],
        removed_from: None,
    },
];

/// The Amazon Linux 2 releases offered for an Amazon EMR release built on it (Amazon
/// EMR 5.30.0 through 6.x). `OSRelease$Label` documents the format: "as shown in Amazon
/// Linux 2 Release Notes. For example, 2.0.20220218.1."
const AMAZON_LINUX_2_OS_RELEASES: &[&str] = &["2.0.20230808.0", "2.0.20231116.0"];

/// The Amazon Linux 2023 releases offered for an Amazon EMR release built on it (Amazon
/// EMR 7.x), in the same dotted release-note form.
const AMAZON_LINUX_2023_OS_RELEASES: &[&str] = &["2023.4.20240429.0", "2023.6.20241010.0"];

/// The first Amazon EMR major release built on Amazon Linux 2023 rather than Amazon
/// Linux 2.
const FIRST_AMAZON_LINUX_2023_EMR_MAJOR: u32 = 7;

/// The Amazon Linux releases available for an Amazon EMR release, by the generation its
/// major release is built on.
fn os_releases_for(major: u32) -> &'static [&'static str] {
    if major >= FIRST_AMAZON_LINUX_2023_EMR_MAJOR {
        AMAZON_LINUX_2023_OS_RELEASES
    } else {
        AMAZON_LINUX_2_OS_RELEASES
    }
}

/// The label of the release `major.minor.patch`, as Amazon EMR spells it.
pub fn release_label(major: u32, minor: u32, patch: u32) -> String {
    format!("emr-{major}.{minor}.{patch}")
}

/// The `(major, minor, patch)` release an `emr-<major>.<minor>.<patch>` label names, or
/// `None` when the label is not in that form.
pub fn release_version(label: &str) -> Option<(u32, u32, u32)> {
    let mut parts = label.strip_prefix("emr-")?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The release `label` names, when the catalog offers it.
///
/// A label has to be spelled exactly as [`release_label`] renders it — so a padded or
/// differently cased variant names no release — AND name a patch release published on
/// one of the lines in [`RELEASE_LINES`].
pub fn catalog_release(label: &str) -> Option<(u32, u32, u32)> {
    let (major, minor, patch) = release_version(label)?;
    if label != release_label(major, minor, patch) {
        return None;
    }
    RELEASE_LINES
        .iter()
        .find(|line| line.major == major && line.minor == minor && line.patches.contains(&patch))
        .map(|_| (major, minor, patch))
}

/// Every release label the catalog offers, NEWEST release first — the order
/// `ListReleaseLabels` reports its first page in ("the first page of results are
/// determined by other filtering parameters or by the latest version",
/// `ListReleaseLabelsInput$NextToken` doc).
pub fn catalog_release_labels() -> Vec<String> {
    let mut releases: Vec<(u32, u32, u32)> = RELEASE_LINES
        .iter()
        .flat_map(|line| {
            line.patches
                .iter()
                .map(move |&patch| (line.major, line.minor, patch))
        })
        .collect();
    releases.sort_unstable_by(|left, right| right.cmp(left));
    releases
        .into_iter()
        .map(|(major, minor, patch)| release_label(major, minor, patch))
        .collect()
}

/// The applications the release line `(major, minor)` bundles, as the wire shapes, in
/// application-name order so every caller sees the same list in the same order.
pub fn release_line_applications(major: u32, minor: u32) -> Vec<SimplifiedApplication> {
    let mut applications: Vec<SimplifiedApplication> = APPLICATION_CATALOG
        .iter()
        .filter_map(|application| {
            Some(SimplifiedApplication {
                name: Some(application.name.to_string()),
                version: Some(application.version_at(major, minor)?.to_string()),
            })
        })
        .collect();
    applications.sort_by(|left, right| left.name.cmp(&right.name));
    applications
}

/// What `DescribeReleaseLabel` reports for one release of the catalog.
#[derive(Debug, Clone)]
pub struct ReleaseLabelRecord {
    pub label: String,
    pub applications: Vec<SimplifiedApplication>,
    pub available_os_releases: Vec<OSRelease>,
}

/// The catalog's entry for `label`, or `None` when it names no release this emulator
/// offers.
pub fn release_label_record(label: &str) -> Option<ReleaseLabelRecord> {
    let (major, minor, patch) = catalog_release(label)?;
    Some(ReleaseLabelRecord {
        label: release_label(major, minor, patch),
        applications: release_line_applications(major, minor),
        available_os_releases: os_releases_for(major)
            .iter()
            .map(|&label| OSRelease {
                label: Some(label.to_string()),
            })
            .collect(),
    })
}

/// The `JobFlowInstancesConfig`-derived facts a cluster reports back.
#[derive(Debug, Clone, Default)]
pub struct InstancesRecord {
    pub instance_collection_type: String,
    pub master_instance_type: Option<String>,
    pub slave_instance_type: Option<String>,
    pub instance_count: Option<i32>,
    pub ec2_key_name: Option<String>,
    pub ec2_subnet_id: Option<String>,
    pub requested_ec2_subnet_ids: Vec<String>,
    pub ec2_availability_zone: Option<String>,
    pub requested_ec2_availability_zones: Vec<String>,
    pub emr_managed_master_security_group: Option<String>,
    pub emr_managed_slave_security_group: Option<String>,
    pub service_access_security_group: Option<String>,
    pub additional_master_security_groups: Vec<String>,
    pub additional_slave_security_groups: Vec<String>,
    pub keep_job_flow_alive_when_no_steps: bool,
    pub termination_protected: bool,
    pub unhealthy_node_replacement: bool,
}

impl InstancesRecord {
    /// Derive the summary from the `Instances` member of a `RunJobFlow` request.
    pub fn from_config(config: &JobFlowInstancesConfig) -> Self {
        let fleets = config
            .instance_fleets
            .as_ref()
            .is_some_and(|fleets| !fleets.is_empty());
        let placement = config.placement.as_ref();
        Self {
            instance_collection_type: if fleets {
                instance_collection_type::INSTANCE_FLEET.to_string()
            } else {
                instance_collection_type::INSTANCE_GROUP.to_string()
            },
            master_instance_type: config.master_instance_type.clone(),
            slave_instance_type: config.slave_instance_type.clone(),
            instance_count: config.instance_count,
            ec2_key_name: config.ec2_key_name.clone(),
            ec2_subnet_id: config.ec2_subnet_id.clone(),
            requested_ec2_subnet_ids: config.ec2_subnet_ids.clone().unwrap_or_default(),
            ec2_availability_zone: placement.and_then(|p| p.availability_zone.clone()),
            requested_ec2_availability_zones: placement
                .and_then(|p| p.availability_zones.clone())
                .unwrap_or_default(),
            emr_managed_master_security_group: config.emr_managed_master_security_group.clone(),
            emr_managed_slave_security_group: config.emr_managed_slave_security_group.clone(),
            service_access_security_group: config.service_access_security_group.clone(),
            additional_master_security_groups: config
                .additional_master_security_groups
                .clone()
                .unwrap_or_default(),
            additional_slave_security_groups: config
                .additional_slave_security_groups
                .clone()
                .unwrap_or_default(),
            // "Specifies whether the cluster should remain available after completing
            // all steps. Defaults to false." (JobFlowInstancesConfig doc.)
            keep_job_flow_alive_when_no_steps: config
                .keep_job_flow_alive_when_no_steps
                .unwrap_or(false),
            termination_protected: config.termination_protected.unwrap_or(false),
            unhealthy_node_replacement: config.unhealthy_node_replacement.unwrap_or(false),
        }
    }
}

/// A cluster's state, its reason, and the instants of its lifecycle.
#[derive(Debug, Clone)]
pub struct ClusterStatusRecord {
    pub state: String,
    pub state_change_reason: Option<StateChangeReasonRecord>,
    pub creation_date_time: SystemTime,
    pub ready_date_time: Option<SystemTime>,
    pub end_date_time: Option<SystemTime>,
    /// When the `STARTING` -> ready transition the clock owes comes due.
    pub ready_due_at: Option<SystemTime>,
    /// When the `TERMINATING` -> `TERMINATED` transition the clock owes comes due.
    pub terminated_due_at: Option<SystemTime>,
}

/// A `(code, message)` state-change reason.
#[derive(Debug, Clone)]
pub struct StateChangeReasonRecord {
    pub code: String,
    pub message: Option<String>,
}

/// One persisted cluster (job flow), built from the `RunJobFlow` request.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub release_label: Option<String>,
    pub requested_ami_version: Option<String>,
    pub log_uri: Option<String>,
    pub log_encryption_kms_key_id: Option<String>,
    pub service_role: Option<String>,
    pub job_flow_role: Option<String>,
    pub auto_scaling_role: Option<String>,
    pub scale_down_behavior: Option<String>,
    pub custom_ami_id: Option<String>,
    pub security_configuration: Option<String>,
    pub os_release_label: Option<String>,
    pub repo_upgrade_on_boot: Option<String>,
    pub visible_to_all_users: bool,
    pub auto_terminate: bool,
    pub step_concurrency_level: i32,
    pub ebs_root_volume_size: Option<i32>,
    pub ebs_root_volume_iops: Option<i32>,
    pub ebs_root_volume_throughput: Option<i32>,
    pub extended_support: Option<bool>,
    pub session_enabled: Option<bool>,
    pub applications: Vec<Application>,
    pub configurations: Vec<Configuration>,
    pub bootstrap_actions: Vec<BootstrapActionConfig>,
    pub placement_groups: Vec<PlacementGroupConfig>,
    pub kerberos_attributes: Option<KerberosAttributes>,
    pub monitoring_configuration: Option<MonitoringConfiguration>,
    pub instances: InstancesRecord,
    /// The managed scaling policy attached to this cluster, by `RunJobFlow` or by
    /// `PutManagedScalingPolicy`, and `None` once `RemoveManagedScalingPolicy` has taken
    /// it off. `GetManagedScalingPolicy` reports exactly this.
    pub managed_scaling_policy: Option<ManagedScalingPolicy>,
    pub tags: Vec<Tag>,
    pub status: ClusterStatusRecord,
    /// Steps in submission order. `ListSteps` reverses this.
    pub steps: Vec<StepRecord>,
    /// The instance fleets this cluster carries, in the order they were added: the
    /// ones `RunJobFlow` was given, then every `AddInstanceFleet`.
    pub fleets: Vec<InstanceFleetRecord>,
    /// The Spark Connect sessions this cluster carries.
    ///
    /// `StartSession` is what appends one; it is still a scaffold stub (its own
    /// issue), so until it lands this is what tells `GetSession` that a session id
    /// names nothing on this cluster. Held as the wire shape for the same reason the
    /// studios are.
    pub sessions: Vec<Session>,
}

impl ClusterRecord {
    /// Materialise every transition the clock already made due, and nothing else.
    ///
    /// Both spans were owed at the same instant whether or not anybody called, so a
    /// reporting operation may apply them: a cluster that was `STARTING` reaches
    /// `WAITING` (it is to stay up after its steps) or `RUNNING`, and one the caller
    /// terminated reaches `TERMINATED`.
    pub fn advance_to(&mut self, now: SystemTime) {
        for fleet in &mut self.fleets {
            fleet.advance_to(now);
        }
        for step in &mut self.steps {
            step.advance_to(now);
        }
        if self.status.state == cluster_state::TERMINATING {
            if let Some(due) = self.status.terminated_due_at {
                if now >= due {
                    self.status.state = cluster_state::TERMINATED.to_string();
                    self.status.end_date_time = Some(due);
                    self.status.terminated_due_at = None;
                }
            }
            return;
        }
        if self.status.state == cluster_state::STARTING {
            if let Some(due) = self.status.ready_due_at {
                if now >= due {
                    self.status.state = if self.instances.keep_job_flow_alive_when_no_steps {
                        cluster_state::WAITING.to_string()
                    } else {
                        cluster_state::RUNNING.to_string()
                    };
                    self.status.ready_date_time = Some(due);
                    self.status.ready_due_at = None;
                }
            }
        }
    }

    /// Start the `TERMINATING` -> `TERMINATED` span, recording why.
    ///
    /// Answers `false` without touching the cluster when it is termination protected
    /// ("Indicates whether Amazon EMR will lock the cluster to prevent the Amazon EC2
    /// instances from being terminated by an API call or user intervention" —
    /// `Cluster$TerminationProtected` doc), and when it is already shutting down or
    /// gone, which makes a repeated `TerminateJobFlows` idempotent.
    pub fn begin_termination(&mut self, now: SystemTime) -> bool {
        if self.instances.termination_protected || self.is_terminal() {
            return false;
        }
        if self.status.state == cluster_state::TERMINATING {
            return false;
        }
        self.status.state = cluster_state::TERMINATING.to_string();
        self.status.state_change_reason = Some(StateChangeReasonRecord {
            code: cluster_state_change_reason_code::USER_REQUEST.to_string(),
            message: Some("Terminated by user request".to_string()),
        });
        self.status.ready_due_at = None;
        self.status.terminated_due_at = Some(now + TRANSITION_SPAN);
        true
    }

    /// Whether this cluster has reached a state it never leaves.
    pub fn is_terminal(&self) -> bool {
        self.status.state == cluster_state::TERMINATED
            || self.status.state == cluster_state::TERMINATED_WITH_ERRORS
    }

    /// The step `step_id` of this cluster, if it has one.
    pub fn step(&self, step_id: &str) -> Option<&StepRecord> {
        self.steps.iter().find(|step| step.id == step_id)
    }

    /// The step `step_id` of this cluster, for an operation that changes it.
    pub fn step_mut(&mut self, step_id: &str) -> Option<&mut StepRecord> {
        self.steps.iter_mut().find(|step| step.id == step_id)
    }

    /// The session `session_id` of this cluster, if it has one.
    pub fn session(&self, session_id: &str) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|session| session.id == session_id)
    }

    /// The wire `ClusterStatus` for this record.
    pub fn to_status(&self) -> ClusterStatus {
        ClusterStatus {
            state: Some(self.status.state.clone()),
            state_change_reason: self.status.state_change_reason.as_ref().map(|reason| {
                ClusterStateChangeReason {
                    code: Some(reason.code.clone()),
                    message: reason.message.clone(),
                }
            }),
            timeline: Some(ClusterTimeline {
                creation_date_time: Some(timestamp(self.status.creation_date_time)),
                ready_date_time: self.status.ready_date_time.map(timestamp),
                end_date_time: self.status.end_date_time.map(timestamp),
            }),
            // Nothing has failed: this cluster carries no error details.
            error_details: None,
        }
    }

    /// The wire `Cluster` for this record — what `DescribeCluster` answers with.
    pub fn to_cluster(&self) -> Cluster {
        Cluster {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.to_status()),
            ec2_instance_attributes: Some(Ec2InstanceAttributes {
                ec2_key_name: self.instances.ec2_key_name.clone(),
                ec2_subnet_id: self.instances.ec2_subnet_id.clone(),
                requested_ec2_subnet_ids: optional_list(&self.instances.requested_ec2_subnet_ids),
                ec2_availability_zone: self.instances.ec2_availability_zone.clone(),
                requested_ec2_availability_zones: optional_list(
                    &self.instances.requested_ec2_availability_zones,
                ),
                // "Also called instance profile and Amazon EC2 role" —
                // RunJobFlow$JobFlowRole is what the cluster's instances assume.
                iam_instance_profile: self.job_flow_role.clone(),
                emr_managed_master_security_group: self
                    .instances
                    .emr_managed_master_security_group
                    .clone(),
                emr_managed_slave_security_group: self
                    .instances
                    .emr_managed_slave_security_group
                    .clone(),
                service_access_security_group: self
                    .instances
                    .service_access_security_group
                    .clone(),
                additional_master_security_groups: optional_list(
                    &self.instances.additional_master_security_groups,
                ),
                additional_slave_security_groups: optional_list(
                    &self.instances.additional_slave_security_groups,
                ),
            }),
            instance_collection_type: Some(self.instances.instance_collection_type.clone()),
            log_uri: self.log_uri.clone(),
            log_encryption_kms_key_id: self.log_encryption_kms_key_id.clone(),
            requested_ami_version: self.requested_ami_version.clone(),
            // No AMI is running: this emulator launches no instances.
            running_ami_version: None,
            release_label: self.release_label.clone(),
            auto_terminate: Some(self.auto_terminate),
            termination_protected: Some(self.instances.termination_protected),
            unhealthy_node_replacement: Some(self.instances.unhealthy_node_replacement),
            visible_to_all_users: Some(self.visible_to_all_users),
            applications: optional_list(&self.applications),
            tags: optional_list(&self.tags),
            service_role: self.service_role.clone(),
            normalized_instance_hours: Some(self.normalized_instance_hours()),
            // No master node exists, so there is no DNS name to report.
            master_public_dns_name: None,
            configurations: optional_list(&self.configurations),
            security_configuration: self.security_configuration.clone(),
            auto_scaling_role: self.auto_scaling_role.clone(),
            scale_down_behavior: self.scale_down_behavior.clone(),
            custom_ami_id: self.custom_ami_id.clone(),
            ebs_root_volume_size: self.ebs_root_volume_size,
            repo_upgrade_on_boot: self.repo_upgrade_on_boot.clone(),
            kerberos_attributes: self.kerberos_attributes.clone(),
            cluster_arn: Some(self.arn.clone()),
            // Not launched on an Outpost.
            outpost_arn: None,
            step_concurrency_level: Some(self.step_concurrency_level),
            placement_groups: optional_list(&self.placement_groups),
            os_release_label: self.os_release_label.clone(),
            ebs_root_volume_iops: self.ebs_root_volume_iops,
            ebs_root_volume_throughput: self.ebs_root_volume_throughput,
            extended_support: self.extended_support,
            monitoring_configuration: self.monitoring_configuration.clone(),
            session_enabled: self.session_enabled,
        }
    }

    /// The wire `ClusterSummary` for this record — what `ListClusters` answers with.
    pub fn to_summary(&self) -> ClusterSummary {
        ClusterSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.to_status()),
            normalized_instance_hours: Some(self.normalized_instance_hours()),
            cluster_arn: Some(self.arn.clone()),
            outpost_arn: None,
        }
    }

    /// "An approximation of the cost of the cluster, represented in m1.small/hours.
    /// This value is incremented one time for every hour an m1.small instance runs."
    /// (`Cluster$NormalizedInstanceHours` doc.) No instance has run for an hour here,
    /// so the count is zero rather than invented.
    fn normalized_instance_hours(&self) -> i32 {
        0
    }
}

/// One persisted instance fleet, built from a submitted `InstanceFleetConfig`.
///
/// `ListInstanceFleets` — still a scaffold stub, its own issue — is what projects this
/// to the modelled `InstanceFleet` shape; what is held here is the submitted
/// configuration plus the id EMR minted for it and the state the clock moves.
#[derive(Debug, Clone)]
pub struct InstanceFleetRecord {
    pub id: String,
    pub config: InstanceFleetConfig,
    pub state: String,
    pub creation_date_time: SystemTime,
    /// When the `PROVISIONING` -> `RUNNING` transition the clock owes comes due.
    pub ready_due_at: Option<SystemTime>,
}

impl InstanceFleetRecord {
    /// Persist a submitted `InstanceFleetConfig` under `id`.
    ///
    /// A fleet EMR has just accepted is `PROVISIONING` — "the instance fleet is
    /// provisioning Amazon EC2 resources and is not yet ready to run jobs"
    /// (`InstanceFleetState`) — and the clock owes it the move to `RUNNING`.
    pub fn from_config(id: String, config: InstanceFleetConfig, now: SystemTime) -> Self {
        Self {
            id,
            config,
            state: instance_fleet_state::PROVISIONING.to_string(),
            creation_date_time: now,
            ready_due_at: Some(now + TRANSITION_SPAN),
        }
    }

    /// Materialise the transition the clock already made due, and nothing else: a
    /// fleet that was `PROVISIONING` is `RUNNING` once its span has passed, which was
    /// owed at that instant whether or not anybody looked.
    pub fn advance_to(&mut self, now: SystemTime) {
        if self.state != instance_fleet_state::PROVISIONING {
            return;
        }
        if let Some(due) = self.ready_due_at {
            if now >= due {
                self.state = instance_fleet_state::RUNNING.to_string();
                self.ready_due_at = None;
            }
        }
    }
}

/// One persisted security configuration, named by a `CreateSecurityConfiguration`
/// request.
///
/// The document is held VERBATIM: `DescribeSecurityConfiguration` "provides the details
/// of a security configuration by returning the configuration JSON" (its Smithy doc),
/// so what a reader gets back is the exact string that was stored, never a
/// re-serialisation of a parse of it.
#[derive(Debug, Clone)]
pub struct SecurityConfigurationRecord {
    pub name: String,
    pub security_configuration: String,
    pub creation_date_time: SystemTime,
}

/// One persisted step, built from a submitted `StepConfig`.
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub id: String,
    pub name: String,
    pub action_on_failure: Option<String>,
    pub jar: String,
    pub main_class: Option<String>,
    pub args: Vec<String>,
    pub properties: BTreeMap<String, String>,
    pub execution_role_arn: Option<String>,
    pub log_uri: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub state: String,
    pub creation_date_time: SystemTime,
    pub start_date_time: Option<SystemTime>,
    pub end_date_time: Option<SystemTime>,
    /// When the `CANCEL_PENDING` -> `CANCELLED` transition the clock owes comes due.
    pub cancelled_due_at: Option<SystemTime>,
}

impl StepRecord {
    /// Persist a submitted `StepConfig` under `id`.
    ///
    /// The step is `PENDING` and stays there: this emulator runs no Hadoop jar, so it
    /// reports no start, no end and no outcome for work nothing performed.
    pub fn from_config(
        id: String,
        config: StepConfig,
        execution_role_arn: Option<String>,
        now: SystemTime,
    ) -> Self {
        let monitoring = config
            .step_monitoring_configuration
            .as_ref()
            .and_then(|monitoring| monitoring.s3_monitoring_configuration.as_ref());
        Self {
            id,
            name: config.name,
            action_on_failure: config.action_on_failure,
            jar: config.hadoop_jar_step.jar,
            main_class: config.hadoop_jar_step.main_class,
            args: config.hadoop_jar_step.args.unwrap_or_default(),
            properties: config
                .hadoop_jar_step
                .properties
                .unwrap_or_default()
                .into_iter()
                .filter_map(|entry| Some((entry.key?, entry.value.unwrap_or_default())))
                .collect(),
            execution_role_arn,
            log_uri: monitoring.and_then(|s3| s3.log_uri.clone()),
            encryption_key_arn: monitoring.and_then(|s3| s3.encryption_key_arn.clone()),
            state: step_state::PENDING.to_string(),
            creation_date_time: now,
            start_date_time: None,
            end_date_time: None,
            cancelled_due_at: None,
        }
    }

    /// Begin cancelling this step on the caller's request.
    ///
    /// Answers `false` without touching the step when it is past cancelling: "you can
    /// cancel steps that are in a `PENDING` or `RUNNING` state" (`CancelSteps` doc),
    /// so a step that has already finished, failed or been cancelled is refused in the
    /// request's own `CancelStepsInfo` rather than silently reported as submitted.
    ///
    /// "CancelSteps is idempotent but asynchronous" (`CancelSteps` doc): the step goes
    /// `CANCEL_PENDING` now and the clock owes it the move to `CANCELLED`, so a second
    /// cancel of the same step reports the same submission and changes nothing.
    pub fn begin_cancellation(&mut self, now: SystemTime) -> bool {
        // "CancelSteps is idempotent but asynchronous" (`CancelSteps` doc): a step
        // whose cancel is already pending was accepted once, so repeating the request
        // reports the same submission and does not push its due instant out.
        if self.state == step_state::CANCEL_PENDING {
            return true;
        }
        if !CANCELLABLE_STEP_STATES.contains(&self.state.as_str()) {
            return false;
        }
        self.state = step_state::CANCEL_PENDING.to_string();
        self.cancelled_due_at = Some(now + TRANSITION_SPAN);
        true
    }

    /// Materialise the transition the clock already made due, and nothing else: a step
    /// the caller cancelled reaches `CANCELLED` once its span has passed.
    pub fn advance_to(&mut self, now: SystemTime) {
        if self.state != step_state::CANCEL_PENDING {
            return;
        }
        if let Some(due) = self.cancelled_due_at {
            if now >= due {
                self.state = step_state::CANCELLED.to_string();
                self.end_date_time = Some(due);
                self.cancelled_due_at = None;
            }
        }
    }

    /// The wire `HadoopStepConfig` for this record.
    fn to_config(&self) -> HadoopStepConfig {
        HadoopStepConfig {
            jar: Some(self.jar.clone()),
            properties: Some(
                self.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<HashMap<String, String>>(),
            ),
            main_class: self.main_class.clone(),
            args: optional_list(&self.args),
        }
    }

    /// The wire `StepStatus` for this record.
    fn to_status(&self) -> StepStatus {
        StepStatus {
            state: Some(self.state.clone()),
            // Nothing moved this step out of PENDING other than a cancel the caller
            // asked for, so there is no reason to give and no failure to detail.
            state_change_reason: None,
            failure_details: None,
            timeline: Some(StepTimeline {
                creation_date_time: Some(timestamp(self.creation_date_time)),
                start_date_time: self.start_date_time.map(timestamp),
                end_date_time: self.end_date_time.map(timestamp),
            }),
        }
    }

    /// The wire `Step` for this record — what `DescribeStep` answers with.
    pub fn to_step(&self) -> Step {
        Step {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            config: Some(self.to_config()),
            action_on_failure: self.action_on_failure.clone(),
            status: Some(self.to_status()),
            execution_role_arn: self.execution_role_arn.clone(),
            log_uri: self.log_uri.clone(),
            encryption_key_arn: self.encryption_key_arn.clone(),
        }
    }

    /// The wire `StepSummary` for this record — what `ListSteps` answers with.
    pub fn to_summary(&self) -> StepSummary {
        StepSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            config: Some(self.to_config()),
            action_on_failure: self.action_on_failure.clone(),
            status: Some(self.to_status()),
            log_uri: self.log_uri.clone(),
            encryption_key_arn: self.encryption_key_arn.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// `ListReleaseLabels`: filtering and paging the release catalog
//
// The catalog itself is the one above ([`RELEASE_LINES`] / [`APPLICATION_CATALOG`],
// read through [`catalog_release_labels`] and [`release_line_applications`]); what
// follows is only what the LISTING needs on top of it — the `Application` filter's
// predicate and the page-token codec. `ListReleaseLabels` "retrieves release labels
// of Amazon EMR services in the Region where the API is called", the same answer for
// every caller, so the listing reads the catalog and writes nothing.

/// "Defines the maximum number of release labels to return in a single
/// response. The default is 100." (`ListReleaseLabelsInput$MaxResults` doc.)
/// The modelled `MaxResultsNumber` range is `1..100`, so 100 is both the
/// default and the largest page a caller can ask for.
pub const LIST_RELEASE_LABELS_MAX_RESULTS: i32 = 100;

/// Whether the release `label` bundles the application a `ReleaseLabelFilter`'s
/// `Application` half names.
///
/// "Optional release label application filter. For example, `spark@2.1.0`."
/// (`ReleaseLabelFilter$Application` doc): the bare name matches whatever version of
/// that application the release bundles, `name@version` only that one version. Both
/// halves are compared case-insensitively, so the documented lowercase form matches
/// however the catalog spells a name. A label the catalog does not offer bundles
/// nothing.
pub fn release_bundles_application(label: &str, filter: &str) -> bool {
    let Some((major, minor, _)) = catalog_release(label) else {
        return false;
    };
    let (name, version) = match filter.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (filter, None),
    };
    release_line_applications(major, minor)
        .iter()
        .any(|application| {
            application
                .name
                .as_deref()
                .is_some_and(|bundled| bundled.eq_ignore_ascii_case(name))
                && version.is_none_or(|wanted| {
                    application
                        .version
                        .as_deref()
                        .is_some_and(|bundled| bundled.eq_ignore_ascii_case(wanted))
                })
        })
}

/// A `NextToken` this listing did not mint for the filters it came back with.
#[derive(Debug, Clone, Copy)]
pub struct InvalidNextToken;

/// The marker every `ListReleaseLabels` page token carries, before the filters
/// it is bound to and the label it resumes after. It is what makes a token
/// verifiable: a string that merely happens to be base64 is not a resume point.
const RELEASE_LABEL_TOKEN_PREFIX: &str = "emr/v1/releaseLabels/";

/// The filters a page token is bound to, rendered with each half's length ahead
/// of it so no two different filter pairs render the same string.
pub fn release_label_filter_fingerprint(
    prefix: Option<&str>,
    application: Option<&str>,
) -> String {
    let prefix = prefix.unwrap_or_default();
    let application = application.unwrap_or_default();
    format!(
        "{}:{prefix}|{}:{application}",
        prefix.len(),
        application.len()
    )
}

/// The opaque token that resumes a `fingerprint`-filtered listing after the
/// release label `cursor`.
pub fn encode_release_label_token(fingerprint: &str, cursor: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{RELEASE_LABEL_TOKEN_PREFIX}{fingerprint}/{cursor}"
    ))
}

/// The release label a token resumes after, or [`InvalidNextToken`] when the
/// token was not minted by this listing for these filters.
///
/// "The ListReleaseLabels request fails if the identity (Amazon Web Services
/// account ID) and all filtering parameters are different from the original
/// request, or if the NextToken is expired or tampered with."
/// (`ListReleaseLabelsInput$NextToken` doc) — binding the filters into the token
/// and checking them back out is what detects both.
pub fn decode_release_label_token(
    token: &str,
    fingerprint: &str,
) -> Result<String, InvalidNextToken> {
    let bytes = ::aws_smithy_types::base64::decode(token).map_err(|_| InvalidNextToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidNextToken)?;
    decoded
        .strip_prefix(RELEASE_LABEL_TOKEN_PREFIX)
        .and_then(|rest| rest.strip_prefix(fingerprint))
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_string)
        .ok_or(InvalidNextToken)
}

/// A stored instant as the wire timestamp the model declares.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Whether `id` can name an Amazon EC2 resource of the kind `prefix` names
/// (`vpc`, `subnet`, `sg`).
///
/// An EC2 resource id is its type's prefix, a hyphen, and then the id itself in
/// lowercase hex — 8 characters in the short form and 17 in the long one ("Resource
/// IDs", Amazon EC2 User Guide). A value outside that form cannot be the id of any EC2
/// resource, so an operation given one is naming a resource that does not exist rather
/// than one it could look up.
pub fn is_ec2_resource_id(prefix: &str, id: &str) -> bool {
    let Some(body) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(body.len(), 8 | 17)
        && body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `location` is an Amazon S3 URI naming at least a bucket (`s3://bucket`,
/// optionally with a key), which is the form every EMR member documented as "the
/// Amazon S3 location …" carries.
pub fn is_s3_location(location: &str) -> bool {
    location
        .strip_prefix("s3://")
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// A list member is absent on the wire when the cluster has none, rather than present
/// and empty — EMR omits what was never supplied.
fn optional_list<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// The tracing target every audit record below is emitted on, so a host can route or
/// filter the audit stream on its own. Keep it equal to the `target:` literal in
/// [`audit`] — a `tracing` target must be a literal, so the two cannot share one name.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured access record one state-changing call owes.
///
/// A security configuration is created and deleted by name, and the effect on the store
/// is otherwise invisible after the fact, so each branch that returns from such a call
/// records it: the operation, the identity the request PRESENTED (its SigV4 access key
/// id, an identifier and not a secret), the request target it named, and the outcome.
/// The subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// `OperationContext::parts()` carries the request's method, URI, headers and
/// extensions, and nothing in this framework puts a peer address in them.
///
/// Deliberately metadata only: no configuration document is logged, so a caller's own
/// data never reaches the log. Every interpolated value is request-derived, so
/// [`audit_field`] bounds it and strips the control characters that would otherwise let
/// one request forge a second record.
pub fn audit(
    operation: &str,
    resource: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        resource = %audit_field(resource),
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        outcome = outcome.as_str(),
        "elasticmapreduce api call"
    );
}

/// Longest request-derived value one audit field carries.
///
/// A configuration name, an access key id and a request path are all caller-controlled,
/// so an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters
/// (above all CR and LF, which would forge a second record) replaced, and the whole
/// thing bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a state-changing call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A security configuration was created and stored.
    SecurityConfigurationCreated,
    /// A stored security configuration was deleted.
    SecurityConfigurationDeleted,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// The name is already taken (400 `InvalidRequestException`).
    Conflict,
    /// A quota — the service's or this emulator's own ceiling — was met.
    QuotaExceeded,
    /// The named resource does not exist.
    NotFound,
    /// The service's own store was unreachable (500).
    StoreUnavailable,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SecurityConfigurationCreated => "security-configuration-created",
            Self::SecurityConfigurationDeleted => "security-configuration-deleted",
            Self::ValidationRefused => "validation-refused",
            Self::Conflict => "conflict",
            Self::QuotaExceeded => "quota-exceeded",
            Self::NotFound => "not-found",
            Self::StoreUnavailable => "store-unavailable",
        }
    }
}

// ---------------------------------------------------------------------------
// The instance-type catalog
//
// The release catalog above ([`RELEASE_LINES`], [`catalog_release`]) says WHICH
// releases this emulator offers; this one says which Amazon EC2 instance types each of
// those releases supports. It is service data in the same way: no operation creates,
// changes or deletes an instance type, so `ListSupportedInstanceTypes` projects the
// table below and writes nothing. Two emulators built from this source answer the same
// catalog, and reading it changes nothing.
// ---------------------------------------------------------------------------

/// `com.amazonaws.emr#SupportedInstanceType$Architecture`: "The CPU architecture, for
/// example `X86_64` or `AARCH64`."
pub mod architecture {
    pub const X86_64: &str = "X86_64";
    pub const AARCH64: &str = "AARCH64";
}

/// One family of the instance-type catalog: an EC2 instance family, the sizes it is
/// offered in, and the facts every size of it shares.
///
/// This is how the catalog is DECLARED. An instance type name is `<family>.<size>`,
/// and within a family vCPUs and memory scale with the size
/// ([`vcpu_for_size`], [`InstanceFamilySpec::memory_gb_per_vcpu`]), so one row per
/// family with its published EC2 size ladder states the whole catalog without hiding
/// a sample behind a hand-picked list of names.
#[derive(Debug, Clone, Copy)]
pub struct InstanceFamilySpec {
    /// The family part of the name, which is also EMR's
    /// `SupportedInstanceType$InstanceFamilyId` ("The Amazon EC2 family and
    /// generation"): `m5`, `c6g`, `r7g`.
    pub family: &'static str,
    /// The size part of every name in this family: its published EC2 size ladder.
    pub sizes: &'static [&'static str],
    /// GiB of memory per vCPU — the constant that makes this family general purpose
    /// (4), compute optimised (2) or memory optimised (8).
    pub memory_gb_per_vcpu: f32,
    /// The `architecture::` value of this family's processors.
    pub architecture: &'static str,
    /// Whether EBS optimization is on by default for this family, rather than merely
    /// available (`SupportedInstanceType$EbsOptimizedByDefault`).
    pub ebs_optimized_by_default: bool,
    /// The FIRST release of each major series that supports this family, as a
    /// `(major, minor, patch)` triple, oldest major first. One entry per major series
    /// the introduction point differs in: Graviton reached `emr-5.31.0` and
    /// `emr-6.1.0`, so it carries both, and a family a major series never offered
    /// simply has no entry for it. See [`InstanceFamilySpec::supported_by`].
    pub introduced_in: &'static [(u32, u32, u32)],
}

impl InstanceFamilySpec {
    /// Whether the release `release` — a `(major, minor, patch)` triple, as
    /// [`catalog_release`] resolves a label to — supports this family.
    ///
    /// Support is carried forward: the first release of a major series that offers a
    /// family is the point it becomes supported in that series, and every later major
    /// series keeps it unless it declares a later introduction point of its own.
    pub fn supported_by(&self, release: (u32, u32, u32)) -> bool {
        let mut carried = false;
        for introduced in self.introduced_in {
            if introduced.0 == release.0 {
                return release >= *introduced;
            }
            if introduced.0 < release.0 {
                carried = true;
            }
        }
        carried
    }

    /// This family's rows: one [`SupportedInstanceType`] per size on its ladder,
    /// smallest first.
    pub fn instance_types(&self) -> Vec<SupportedInstanceType> {
        self.sizes
            .iter()
            .filter_map(|size| {
                let scaled = vcpu_for_size(size);
                // A ladder entry this catalog cannot scale would silently shrink a
                // release's list, so a typo in the table fails the tests rather than
                // quietly dropping a row.
                debug_assert!(
                    scaled.is_some(),
                    "catalog size {size:?} of family {} must scale",
                    self.family
                );
                let vcpu = scaled?;
                Some(SupportedInstanceType {
                    r#type: Some(format!("{}.{size}", self.family)),
                    memory_gb: Some(vcpu as f32 * self.memory_gb_per_vcpu),
                    // "StorageGB represents the storage capacity … This value is 0
                    // for Amazon EBS-only instance types." Every family in this
                    // catalog is EBS-only, so is every row.
                    storage_gb: Some(0),
                    vcpu: Some(vcpu),
                    // Every family here runs a 64-bit processor only.
                    is64_bits_only: Some(true),
                    instance_family_id: Some(self.family.to_string()),
                    ebs_optimized_available: Some(true),
                    ebs_optimized_by_default: Some(self.ebs_optimized_by_default),
                    // "Number of disks … This value is 0 for Amazon EBS-only
                    // instance types."
                    number_of_disks: Some(0),
                    ebs_storage_only: Some(true),
                    architecture: Some(self.architecture.to_string()),
                })
            })
            .collect()
    }
}

/// GiB of memory per vCPU of a general-purpose (`m`) family.
const MEMORY_GENERAL_PURPOSE: f32 = 4.0;
/// GiB of memory per vCPU of a compute-optimised (`c`) family.
const MEMORY_COMPUTE_OPTIMIZED: f32 = 2.0;
/// GiB of memory per vCPU of a memory-optimised (`r`) family.
const MEMORY_MEMORY_OPTIMIZED: f32 = 8.0;

/// The size ladder of the fifth-generation Intel families (`m5`, `r5`) and the AMD
/// ones (`m5a`, `r5a`).
const SIZES_5TH_GENERATION: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
];
/// The `c5` ladder, which steps through `9xlarge` and `18xlarge`.
const SIZES_C5: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "9xlarge", "12xlarge", "18xlarge", "24xlarge",
];
/// The sixth- and seventh-generation Intel ladder (`m6i`, `c6i`, `r6i`), which adds
/// `32xlarge`.
const SIZES_6TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge",
];
/// The sixth-generation AMD ladder (`m6a`, `c6a`, `r6a`), which goes up to `48xlarge`.
const SIZES_6TH_GENERATION_AMD: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge", "48xlarge",
];
/// The seventh-generation Intel ladder (`m7i`, `c7i`, `r7i`), which steps from
/// `24xlarge` to `48xlarge`.
const SIZES_7TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "48xlarge",
];
/// The Graviton ladder (`m6g`, `c6g`, `r6g`, `m7g`, `c7g`, `r7g`) from `large` up.
/// The sub-`large` `medium` size each of these families also comes in is outside this
/// catalog: EMR runs no cluster node smaller than `large`.
const SIZES_GRAVITON: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
];

/// The instance families this emulator's EMR catalog publishes, each with the sizes
/// it is offered in, its memory ratio, its architecture and the release it became
/// supported in.
///
/// SOURCE: the Amazon EMR Management Guide's "Supported instance types" table (which
/// instance types each release supports) crossed with each family's published EC2
/// size ladder above. Membership and the introduction points are catalog data, not
/// modelled facts: the model enumerates no instance type at all. Refresh them from
/// that page.
///
/// What is deliberately OUTSIDE it: instance-store families (`m5d`, `i3`, `d3`, `z1d`
/// …) and the GPU/accelerator families, because this catalog publishes no per-size
/// instance-store disk table and would have to invent `StorageGB`/`NumberOfDisks` for
/// them; burstable (`t`) families and sizes below `large`, which EMR does not run
/// cluster nodes on; previous-generation families (`m1`…`m4`, `c1`…`c4`, `r3`, `r4`);
/// and bare-metal sizes.
pub const INSTANCE_FAMILIES: &[InstanceFamilySpec] = &[
    // General purpose, x86-64.
    InstanceFamilySpec {
        family: "m5",
        sizes: SIZES_5TH_GENERATION,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 13, 0)],
    },
    InstanceFamilySpec {
        family: "m5a",
        sizes: SIZES_5TH_GENERATION,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 20, 0)],
    },
    InstanceFamilySpec {
        family: "m6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    InstanceFamilySpec {
        family: "m6a",
        sizes: SIZES_6TH_GENERATION_AMD,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 6, 0)],
    },
    InstanceFamilySpec {
        family: "m7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // General purpose, Graviton.
    InstanceFamilySpec {
        family: "m6g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 31, 0), (6, 1, 0)],
    },
    InstanceFamilySpec {
        family: "m7g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // Compute optimised.
    InstanceFamilySpec {
        family: "c5",
        sizes: SIZES_C5,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 13, 0)],
    },
    InstanceFamilySpec {
        family: "c6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    InstanceFamilySpec {
        family: "c7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    InstanceFamilySpec {
        family: "c6g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 31, 0), (6, 1, 0)],
    },
    InstanceFamilySpec {
        family: "c7g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // Memory optimised.
    InstanceFamilySpec {
        family: "r5",
        sizes: SIZES_5TH_GENERATION,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 20, 0)],
    },
    InstanceFamilySpec {
        family: "r6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    InstanceFamilySpec {
        family: "r6g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 31, 0), (6, 1, 0)],
    },
    InstanceFamilySpec {
        family: "r7g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
];

/// The vCPU count of the size `size` on an EC2 size ladder, or `None` for a size this
/// catalog does not scale (`medium`, `metal`).
///
/// EC2 sizes a family in units of two vCPUs: `large` is 2, `xlarge` is 4, and
/// `<n>xlarge` is `4 × n` — `m5.24xlarge`'s 96 vCPUs, say.
pub fn vcpu_for_size(size: &str) -> Option<i32> {
    const VCPU_PER_XLARGE: i32 = 4;
    match size {
        "large" => Some(VCPU_PER_XLARGE / 2),
        "xlarge" => Some(VCPU_PER_XLARGE),
        _ => {
            let multiple = size.strip_suffix("xlarge")?;
            if multiple.is_empty() || !multiple.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            multiple
                .parse::<i32>()
                .ok()
                // A multiple no EC2 ladder reaches is refused rather than wrapped:
                // the size is not one this catalog can scale.
                .and_then(|multiple| multiple.checked_mul(VCPU_PER_XLARGE))
        }
    }
}

/// The instance types the release `release` — a `(major, minor, patch)` triple, as
/// [`catalog_release`] resolves a label to — supports, in catalog order: by family as
/// [`INSTANCE_FAMILIES`] declares them, and within a family smallest size first.
pub fn supported_instance_types(release: (u32, u32, u32)) -> Vec<SupportedInstanceType> {
    INSTANCE_FAMILIES
        .iter()
        .filter(|family| family.supported_by(release))
        .flat_map(InstanceFamilySpec::instance_types)
        .collect()
}

/// `ListSupportedInstanceTypes` page size.
///
/// CHOSEN, not documented: the operation is paginated on `Marker` but AWS publishes no
/// page size for it. 100 matches the maximum page the service's other paginated
/// catalog reader models (`MaxResultsNumber`, `range 1..100`), so a caller that
/// follows the marker walks the whole catalog in pages of a familiar size.
pub const LIST_SUPPORTED_INSTANCE_TYPES_PAGE_SIZE: usize = 100;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEmrElasticmapreduceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEmrElasticmapreduceState>)
}
