// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice service.
//!
//! This is the single store every handler in `src/ops/` reads and writes: the
//! applications, the application versions and the environments the lifecycle
//! operations create, describe and delete, plus the managed platform branch
//! catalog the service publishes. It also owns the few derivations all
//! of them share — the server clock, the request's region, this service's ARN
//! forms, and the clock-owed environment status transitions — so no handler
//! spells one of them a second way.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    ApplicationDescription, ApplicationResourceLifecycleConfig, ApplicationVersionDescription,
    ConfigurationOptionSetting, EnvironmentTier, PlatformBranchSummary, S3Location, Tag,
};

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`arnNamespace: elasticbeanstalk`).
const ARN_NAMESPACE: &str = "elasticbeanstalk";

/// The region an ARN and a CNAME are built in when the request carries no SigV4
/// credential scope to read one from (an unsigned request to the emulator).
const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a status change this service owes
/// becomes due: `Launching` -> `Ready` after CreateEnvironment, `Terminating` ->
/// `Terminated` after TerminateEnvironment, `Aborting` -> `Ready` after
/// AbortEnvironmentUpdate.
///
/// The real service takes minutes, so this span is CHOSEN, not documented. It is
/// deliberately non-zero: a caller that describes the environment straight after
/// creating it must still observe the transient status the real service reports.
/// The due instant is stored on the record and materialised when a later call
/// observes it — nothing sleeps.
pub const STATUS_TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `EnvironmentStatus` values this service writes. The full modelled value set is
/// `crate::ops::types::ENVIRONMENT_STATUS_VALUES`.
pub const STATUS_LAUNCHING: &str = "Launching";
/// See [`STATUS_LAUNCHING`].
pub const STATUS_READY: &str = "Ready";
/// See [`STATUS_LAUNCHING`].
pub const STATUS_TERMINATING: &str = "Terminating";
/// See [`STATUS_LAUNCHING`].
pub const STATUS_TERMINATED: &str = "Terminated";
/// See [`STATUS_LAUNCHING`]. "Aborting: Environment is in the process of
/// aborting a deployment."
pub const STATUS_ABORTING: &str = "Aborting";

/// `EnvironmentHealth` values this service writes (modelled set:
/// `crate::ops::types::ENVIRONMENT_HEALTH_VALUES`). `Grey` is the value the model
/// documents as the "Default health for a new environment … not fully launched
/// and health checks have not started"; `Green` is "the environment is healthy
/// and fully functional", which is what the launch this emulator performs ends in.
pub const HEALTH_GREY: &str = "Grey";
/// See [`HEALTH_GREY`].
pub const HEALTH_GREEN: &str = "Green";

/// `EnvironmentHealthStatus` values this service writes (modelled set:
/// `crate::ops::types::ENVIRONMENT_HEALTH_STATUS_VALUES`).
pub const HEALTH_STATUS_PENDING: &str = "Pending";
/// See [`HEALTH_STATUS_PENDING`].
pub const HEALTH_STATUS_OK: &str = "Ok";

/// The support life-cycle states a platform branch reports, from
/// `PlatformBranchSummary$LifecycleState`'s own documentation ("Possible values:
/// `beta` | `supported` | `deprecated` | `retired`"). The model types that
/// member as a plain string with no enum trait, so the value set is named here.
/// Sorted, so a message that lists them reads the same every time.
pub const PLATFORM_BRANCH_LIFECYCLE_STATES: &[&str] =
    &["beta", "deprecated", "retired", "supported"];

/// The environment tiers a platform branch can support, from
/// `PlatformBranchSummary$SupportedTierList`'s documentation ("Possible values:
/// `WebServer/Standard` | `Worker/SQS/HTTP`"). Also a plain string in the model.
pub const SUPPORTED_TIERS: &[&str] = &[TIER_WEB_SERVER, TIER_WORKER];

/// See [`SUPPORTED_TIERS`].
pub const TIER_WEB_SERVER: &str = "WebServer/Standard";
/// See [`SUPPORTED_TIERS`].
pub const TIER_WORKER: &str = "Worker/SQS/HTTP";

/// Both tiers, for the catalog table below.
const BOTH_TIERS: &[&str] = &[TIER_WEB_SERVER, TIER_WORKER];
/// Web server tier only (the Windows Server branches, which run no worker
/// tier), for the catalog table below.
const WEB_SERVER_ONLY: &[&str] = &[TIER_WEB_SERVER];

/// The only `ApplicationVersionStatus` this service reports: it runs no version
/// processing pipeline, so a stored version is never `Processing`/`Processed`.
pub const VERSION_STATUS_UNPROCESSED: &str = "Unprocessed";

/// Elastic Beanstalk's published default account quotas, which bound this store:
/// 75 applications, 1,000 application versions and 200 environments per account
/// per region. Enforcing them is also what makes the three `TooMany…Exception`
/// errors the model declares reachable.
pub const MAX_APPLICATIONS: usize = 75;
/// See [`MAX_APPLICATIONS`].
pub const MAX_APPLICATION_VERSIONS: usize = 1_000;
/// See [`MAX_APPLICATIONS`]. Covers environments that have not reached
/// `Terminated`, which is what the real quota counts.
pub const MAX_ENVIRONMENTS: usize = 200;

/// How many terminated environments the store keeps, so DescribeEnvironments can
/// still report them for `IncludeDeleted`.
///
/// A terminated environment no longer counts against [`MAX_ENVIRONMENTS`], so
/// without a retention bound a create/terminate loop would grow the store
/// forever. The bound is a COUNT, not a duration, so the store's size is capped
/// outright; the count is chosen, not documented. The most recently terminated
/// records are the ones kept.
pub const MAX_RETAINED_TERMINATED_ENVIRONMENTS: usize = 200;

/// The server clock, as the modelled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request is addressed to: the region field of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it carries none.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:application/<name>`.
pub fn application_arn(region: &str, application_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("application/{application_name}"),
    )
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:applicationversion/<app>/<label>`.
pub fn application_version_arn(region: &str, application_name: &str, version_label: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("applicationversion/{application_name}/{version_label}"),
    )
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:environment/<app>/<env name>`.
pub fn environment_arn(region: &str, application_name: &str, environment_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("environment/{application_name}/{environment_name}"),
    )
}

/// The DNS name an environment is reached at:
/// `<cname prefix>.<region>.elasticbeanstalk.com`.
pub fn environment_cname(cname_prefix: &str, region: &str) -> String {
    format!("{cname_prefix}.{region}.elasticbeanstalk.com")
}

/// A stored application.
#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub resource_lifecycle_config: Option<ApplicationResourceLifecycleConfig>,
    /// Template names, carried so DescribeApplications reports them; creating a
    /// configuration template is its own (still scaffolded) operation.
    pub configuration_templates: Vec<String>,
    pub tags: Vec<Tag>,
}

impl Application {
    /// The modelled `ApplicationDescription` for this record. `versions` is
    /// derived by the caller from [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::version_labels`],
    /// so the list is never a second copy of the version store.
    pub fn to_description(&self, versions: Vec<String>) -> ApplicationDescription {
        ApplicationDescription {
            application_arn: Some(self.arn.clone()),
            application_name: Some(self.name.clone()),
            description: self.description.clone(),
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            versions: Some(versions),
            configuration_templates: Some(self.configuration_templates.clone()),
            resource_lifecycle_config: self.resource_lifecycle_config.clone(),
        }
    }
}

/// A stored application version.
#[derive(Debug, Clone)]
pub struct ApplicationVersion {
    pub application_name: String,
    pub version_label: String,
    pub arn: String,
    pub description: Option<String>,
    pub source_bundle: Option<S3Location>,
    /// What the caller asked for in `Process`, echoed back. Nothing processes the
    /// bundle here, so [`Self::status`] stays `Unprocessed` either way.
    pub process: Option<bool>,
    pub status: String,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
    /// Creation order within this emulator, so "newest first" is a total order
    /// even for versions created inside one clock tick.
    pub sequence: u64,
}

impl ApplicationVersion {
    /// The modelled `ApplicationVersionDescription` for this record.
    pub fn to_description(&self) -> ApplicationVersionDescription {
        ApplicationVersionDescription {
            application_version_arn: Some(self.arn.clone()),
            application_name: Some(self.application_name.clone()),
            description: self.description.clone(),
            version_label: Some(self.version_label.clone()),
            source_build_information: None,
            build_arn: None,
            source_bundle: self.source_bundle.clone(),
            image_source: None,
            image_build_configuration: None,
            process: self.process,
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            status: Some(self.status.clone()),
        }
    }
}

/// A platform branch: one line of the managed platform catalog Elastic
/// Beanstalk publishes for an account in a region, which is what
/// `ListPlatformBranches` reports.
#[derive(Debug, Clone)]
pub struct PlatformBranch {
    pub platform_name: String,
    pub branch_name: String,
    /// One of [`PLATFORM_BRANCH_LIFECYCLE_STATES`].
    pub lifecycle_state: String,
    /// "A larger `BranchOrder` value designates a newer platform branch within
    /// the platform."
    pub branch_order: i32,
    /// A subset of [`SUPPORTED_TIERS`].
    pub supported_tiers: Vec<String>,
}

impl PlatformBranch {
    /// The identity of a branch in the catalog: a branch name is unique within
    /// its platform, so the pair is the store's key and the value a
    /// `NextToken` names.
    pub fn key(&self) -> String {
        format!("{}/{}", self.platform_name, self.branch_name)
    }

    /// Whether `token` is this branch's [`Self::key`] — the same test, without
    /// building the key, for the page-boundary scan.
    pub fn has_key(&self, token: &str) -> bool {
        token
            .strip_prefix(self.platform_name.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| rest == self.branch_name)
    }

    /// The modelled `PlatformBranchSummary` for this record.
    pub fn to_summary(&self) -> PlatformBranchSummary {
        PlatformBranchSummary {
            platform_name: Some(self.platform_name.clone()),
            branch_name: Some(self.branch_name.clone()),
            lifecycle_state: Some(self.lifecycle_state.clone()),
            branch_order: self.branch_order,
            supported_tier_list: Some(self.supported_tiers.clone()),
        }
    }
}

/// The managed platform branches an account can see in a region, as
/// `(PlatformName, BranchName, LifecycleState, BranchOrder, SupportedTierList)`.
///
/// Elastic Beanstalk's platform catalog is service-owned reference data, not
/// something a caller creates, so it is seeded into the store by
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::default`] and
/// read from there like any other record. The names and the solution-stack
/// generations are Elastic Beanstalk's own published platform branches; the
/// life-cycle states span all four of [`PLATFORM_BRANCH_LIFECYCLE_STATES`] and
/// the tier lists both of [`SUPPORTED_TIERS`], because the real catalog does
/// and a caller filtering on either must have something to match. Which
/// generation sits in which state moves whenever AWS retires one, so this
/// table is the emulator's catalog rather than a snapshot of any one day's.
#[allow(clippy::type_complexity)]
const MANAGED_PLATFORM_BRANCHES: &[(&str, &str, &str, i32, &[&str])] = &[
    // .NET on Linux.
    (
        ".NET Core on Linux",
        ".NET 9 running on 64bit Amazon Linux 2023",
        "beta",
        2,
        BOTH_TIERS,
    ),
    (
        ".NET Core on Linux",
        ".NET 8 running on 64bit Amazon Linux 2023",
        "supported",
        1,
        BOTH_TIERS,
    ),
    (
        ".NET Core on Linux",
        ".NET Core running on 64bit Amazon Linux 2",
        "deprecated",
        0,
        BOTH_TIERS,
    ),
    // .NET on Windows Server: no worker tier runs on Windows.
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server 2019",
        "supported",
        1,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server Core 2016",
        "retired",
        0,
        WEB_SERVER_ONLY,
    ),
    // Docker.
    (
        "Docker",
        "Docker running on 64bit Amazon Linux 2023",
        "supported",
        2,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "Docker running on 64bit Amazon Linux 2",
        "deprecated",
        1,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "Multi-container Docker running on 64bit Amazon Linux",
        "retired",
        0,
        BOTH_TIERS,
    ),
    // Go.
    (
        "Go",
        "Go 1 running on 64bit Amazon Linux 2023",
        "supported",
        1,
        BOTH_TIERS,
    ),
    (
        "Go",
        "Go 1 running on 64bit Amazon Linux 2",
        "retired",
        0,
        BOTH_TIERS,
    ),
    // Java SE (Corretto).
    (
        "Corretto",
        "Corretto 21 running on 64bit Amazon Linux 2023",
        "supported",
        2,
        BOTH_TIERS,
    ),
    (
        "Corretto",
        "Corretto 17 running on 64bit Amazon Linux 2023",
        "supported",
        1,
        BOTH_TIERS,
    ),
    (
        "Corretto",
        "Corretto 11 running on 64bit Amazon Linux 2",
        "deprecated",
        0,
        BOTH_TIERS,
    ),
    // Node.js.
    (
        "Node.js",
        "Node.js 22 running on 64bit Amazon Linux 2023",
        "supported",
        3,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 20 running on 64bit Amazon Linux 2023",
        "supported",
        2,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 18 running on 64bit Amazon Linux 2023",
        "deprecated",
        1,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 16 running on 64bit Amazon Linux 2",
        "retired",
        0,
        BOTH_TIERS,
    ),
    // PHP.
    (
        "PHP",
        "PHP 8.4 running on 64bit Amazon Linux 2023",
        "supported",
        2,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.3 running on 64bit Amazon Linux 2023",
        "supported",
        1,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.1 running on 64bit Amazon Linux 2",
        "retired",
        0,
        BOTH_TIERS,
    ),
    // Python.
    (
        "Python",
        "Python 3.13 running on 64bit Amazon Linux 2023",
        "supported",
        3,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.11 running on 64bit Amazon Linux 2023",
        "supported",
        2,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.9 running on 64bit Amazon Linux 2023",
        "deprecated",
        1,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.8 running on 64bit Amazon Linux 2",
        "retired",
        0,
        BOTH_TIERS,
    ),
    // Ruby.
    (
        "Ruby",
        "Ruby 3.4 running on 64bit Amazon Linux 2023",
        "supported",
        1,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 3.2 running on 64bit Amazon Linux 2023",
        "deprecated",
        0,
        BOTH_TIERS,
    ),
    // Tomcat.
    (
        "Tomcat",
        "Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023",
        "supported",
        2,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023",
        "supported",
        1,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2",
        "retired",
        0,
        BOTH_TIERS,
    ),
];

/// [`MANAGED_PLATFORM_BRANCHES`] as stored records, keyed by
/// [`PlatformBranch::key`].
fn managed_platform_branch_catalog() -> BTreeMap<String, PlatformBranch> {
    MANAGED_PLATFORM_BRANCHES
        .iter()
        .map(
            |(platform_name, branch_name, lifecycle_state, branch_order, supported_tiers)| {
                let branch = PlatformBranch {
                    platform_name: (*platform_name).to_string(),
                    branch_name: (*branch_name).to_string(),
                    lifecycle_state: (*lifecycle_state).to_string(),
                    branch_order: *branch_order,
                    supported_tiers: supported_tiers
                        .iter()
                        .map(|tier| (*tier).to_string())
                        .collect(),
                };
                (branch.key(), branch)
            },
        )
        .collect()
}

/// A status change the clock owes an environment: what it moves to, and when
/// that move is due. See [`STATUS_TRANSITION_SPAN`].
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub due_at: Instant,
    pub status: String,
    pub health: String,
    pub health_status: Option<String>,
}

impl PendingStatus {
    /// The move an in-progress operation on a surviving environment owes once it
    /// finishes: to `Ready` ("Environment is available to have an action
    /// performed on it"), reporting healthy ("Green: Indicates the environment
    /// is healthy and fully functional"). Both the launch CreateEnvironment
    /// starts and the cancellation AbortEnvironmentUpdate starts end here, so
    /// the two cannot drift apart.
    pub fn completion_to_ready() -> Self {
        Self {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            status: STATUS_READY.to_string(),
            health: HEALTH_GREEN.to_string(),
            health_status: Some(HEALTH_STATUS_OK.to_string()),
        }
    }
}

/// A stored environment.
#[derive(Debug, Clone)]
pub struct Environment {
    pub environment_id: String,
    pub environment_name: String,
    pub arn: String,
    pub application_name: String,
    pub version_label: Option<String>,
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    pub template_name: Option<String>,
    pub description: Option<String>,
    pub cname: String,
    pub endpoint_url: Option<String>,
    pub tier: Option<EnvironmentTier>,
    pub option_settings: Vec<ConfigurationOptionSetting>,
    pub operations_role: Option<String>,
    pub status: String,
    pub health: String,
    pub health_status: Option<String>,
    pub abortable_operation_in_progress: bool,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    /// When the record reached `Terminated`, for DescribeEnvironments'
    /// `IncludedDeletedBackTo` filter.
    pub date_terminated: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    pub pending_status: Option<PendingStatus>,
}

impl Environment {
    /// Whether this environment has reached `Terminated` — the state
    /// DescribeEnvironments hides unless the caller asks for deleted ones.
    pub fn is_terminated(&self) -> bool {
        self.status == STATUS_TERMINATED
    }

    /// Start the move to `Terminating`, with the move to `Terminated` due one
    /// [`STATUS_TRANSITION_SPAN`] from now.
    pub fn begin_termination(&mut self) {
        self.status = STATUS_TERMINATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = false;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            status: STATUS_TERMINATED.to_string(),
            health: HEALTH_GREY.to_string(),
            // A terminated environment runs nothing, so it reports no health
            // status at all rather than one of the running values.
            health_status: None,
        });
    }

    /// Start the cancellation of the in-progress update this environment
    /// reports: the environment moves to `Aborting` ("Environment is in the
    /// process of aborting a deployment"), stops reporting an abortable
    /// operation — the one in flight is already being cancelled — and the
    /// return to `Ready` is what the clock then owes, one
    /// [`STATUS_TRANSITION_SPAN`] later.
    ///
    /// Health goes `Grey`/`Pending` for the duration, as the model documents
    /// for an environment whose "health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request".
    pub fn begin_abort(&mut self) {
        self.status = STATUS_ABORTING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = false;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus::completion_to_ready());
    }

    /// Apply the pending status change if the clock has already made it due.
    ///
    /// This is the one write a reporting operation performs: the change was owed
    /// at `due_at` whether or not anybody called, so materialising it on read
    /// cannot show a caller a state the clock had not already reached.
    fn materialize_due(&mut self, at: Instant) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if pending.due_at > at {
            return;
        }
        let pending = self.pending_status.take().expect("checked above");
        self.status = pending.status;
        self.health = pending.health;
        self.health_status = pending.health_status;
        self.date_updated = now();
        if self.status == STATUS_TERMINATED {
            self.date_terminated = Some(self.date_updated);
        }
    }
}

/// Build one of the generated `EnvironmentDescription` structs from a stored
/// environment.
///
/// The model binds `EnvironmentDescription` as the output of several operations
/// and the scaffold emits a DISTINCT Rust struct per operation for it
/// (`CreateEnvironmentOutput`, `TerminateEnvironmentOutput`, …) with identical
/// members, so the mapping is written once here and the struct is the parameter.
macro_rules! environment_description {
    ($ty:path, $env:expr) => {{
        let env: &$crate::state::Environment = $env;
        $ty {
            environment_name: Some(env.environment_name.clone()),
            environment_id: Some(env.environment_id.clone()),
            application_name: Some(env.application_name.clone()),
            version_label: env.version_label.clone(),
            solution_stack_name: env.solution_stack_name.clone(),
            platform_arn: env.platform_arn.clone(),
            template_name: env.template_name.clone(),
            description: env.description.clone(),
            endpoint_url: env.endpoint_url.clone(),
            cname: Some(env.cname.clone()),
            date_created: Some(env.date_created),
            date_updated: Some(env.date_updated),
            status: Some(env.status.clone()),
            abortable_operation_in_progress: Some(env.abortable_operation_in_progress),
            health: Some(env.health.clone()),
            health_status: env.health_status.clone(),
            // No load balancer or EC2 instance is provisioned here, so there are
            // no environment resources and no environment links to report.
            resources: None,
            tier: env.tier.clone(),
            environment_links: None,
            environment_arn: Some(env.arn.clone()),
            operations_role: env.operations_role.clone(),
        }
    }};
}

pub(crate) use environment_description;

/// Per-service state for com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// Applications by `ApplicationName` (unique within an account).
    pub applications: BTreeMap<String, Application>,
    /// Application versions by `(ApplicationName, VersionLabel)`.
    pub application_versions: BTreeMap<(String, String), ApplicationVersion>,
    /// Environments by `EnvironmentId`.
    pub environments: BTreeMap<String, Environment>,
    /// `EnvironmentName` -> `EnvironmentId`, so an operation that takes either
    /// one resolves the same record.
    pub environment_name_index: BTreeMap<String, String>,
    /// The managed platform branches this account can see, by
    /// [`PlatformBranch::key`]. Service-owned reference data: seeded from
    /// [`MANAGED_PLATFORM_BRANCHES`] when the store is created, so every
    /// platform operation reads one catalog.
    pub platform_branches: BTreeMap<String, PlatformBranch>,
    /// Source of unique `EnvironmentId` values and of version sequence numbers.
    pub next_id_counter: u64,
}

/// The store a fresh engine starts from: no applications, versions or
/// environments, and the managed platform catalog already published — the
/// latter is service-owned data no caller creates, so `ListPlatformBranches`
/// reads it rather than writing it on first use.
impl Default for ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    fn default() -> Self {
        Self {
            applications: BTreeMap::new(),
            application_versions: BTreeMap::new(),
            environments: BTreeMap::new(),
            environment_name_index: BTreeMap::new(),
            platform_branches: managed_platform_branch_catalog(),
            next_id_counter: 0,
        }
    }
}

impl ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// A fresh unique `EnvironmentId`, in the `e-<8 lowercase alphanumerics>`
    /// form the real service issues.
    pub fn next_environment_id(&mut self) -> String {
        format!("e-{:0>8}", base36(self.next_counter()))
    }

    /// The next value of the shared counter.
    pub fn next_counter(&mut self) -> u64 {
        self.next_id_counter += 1;
        self.next_id_counter
    }

    /// An application's version labels, newest first.
    pub fn version_labels(&self, application_name: &str) -> Vec<String> {
        let mut versions: Vec<&ApplicationVersion> = self
            .application_versions
            .values()
            .filter(|version| version.application_name == application_name)
            .collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version.sequence));
        versions
            .into_iter()
            .map(|version| version.version_label.clone())
            .collect()
    }

    /// The id of the environment named by `EnvironmentId` or `EnvironmentName`,
    /// whichever the caller supplied.
    pub fn resolve_environment_id(
        &self,
        environment_id: Option<&str>,
        environment_name: Option<&str>,
    ) -> Option<String> {
        if let Some(id) = environment_id {
            if self.environments.contains_key(id) {
                return Some(id.to_string());
            }
            return None;
        }
        let name = environment_name?;
        self.environment_name_index.get(name).cloned()
    }

    /// Every platform branch in the catalog, in the one order
    /// `ListPlatformBranches` reports and pages through: by platform name, then
    /// newest branch first within a platform (`BranchOrder`: "A larger
    /// BranchOrder value designates a newer platform branch within the
    /// platform"), then by branch name so the order is total and a page
    /// boundary falls in the same place on every call.
    pub fn ordered_platform_branches(&self) -> Vec<&PlatformBranch> {
        let mut branches: Vec<&PlatformBranch> = self.platform_branches.values().collect();
        branches.sort_by(|left, right| {
            left.platform_name
                .cmp(&right.platform_name)
                .then(right.branch_order.cmp(&left.branch_order))
                .then(left.branch_name.cmp(&right.branch_name))
        });
        branches
    }

    /// How many environments have not reached `Terminated` — the count the
    /// environment quota covers.
    pub fn running_environment_count(&self) -> usize {
        self.environments
            .values()
            .filter(|environment| !environment.is_terminated())
            .count()
    }

    /// Materialise every environment status change the clock has already made
    /// due. Every handler that reads or acts on an environment calls this first,
    /// so no two of them disagree about what state the store is in.
    pub fn materialize_due_transitions(&mut self) {
        let at = Instant::now();
        for environment in self.environments.values_mut() {
            environment.materialize_due(at);
        }
        self.evict_retired_environments();
    }

    /// Hold the retention bound on terminated environments: past
    /// [`MAX_RETAINED_TERMINATED_ENVIRONMENTS`] the oldest terminated records are
    /// dropped, so repeated create/terminate cycles cannot grow the store without
    /// bound. Runs with the clock materialisation above, so it is applied on
    /// every call that touches an environment.
    fn evict_retired_environments(&mut self) {
        let mut retired: Vec<(i64, String)> = self
            .environments
            .values()
            .filter(|environment| environment.is_terminated())
            .map(|environment| {
                (
                    environment.date_terminated.map_or(0, |at| at.secs()),
                    environment.environment_id.clone(),
                )
            })
            .collect();
        if retired.len() <= MAX_RETAINED_TERMINATED_ENVIRONMENTS {
            return;
        }
        // Oldest first; the id breaks a tie deterministically.
        retired.sort();
        let excess = retired.len() - MAX_RETAINED_TERMINATED_ENVIRONMENTS;
        for (_, environment_id) in retired.into_iter().take(excess) {
            if let Some(environment) = self.environments.remove(&environment_id) {
                // Release the name only if it still resolves to this record.
                if self
                    .environment_name_index
                    .get(&environment.environment_name)
                    == Some(&environment.environment_id)
                {
                    self.environment_name_index
                        .remove(&environment.environment_name);
                }
            }
        }
    }
}

/// The message this service refuses a request with `InvalidParameterValue` for
/// when the identifier it supplied names no stored environment, built from the
/// request: it names the member the caller used and the value it carried, the
/// way the real service's "No Environment found for EnvironmentName = '…'."
/// does. Shared by every operation that takes the `EnvironmentId` /
/// `EnvironmentName` either-or pair, so they cannot disagree.
pub fn missing_environment_message(
    environment_id: Option<&str>,
    environment_name: Option<&str>,
) -> String {
    match (environment_id, environment_name) {
        (Some(id), _) => format!("No Environment found for EnvironmentId = '{id}'."),
        (None, Some(name)) => format!("No Environment found for EnvironmentName = '{name}'."),
        // Neither supplied is a MissingRequiredParameter refusal, which every
        // caller makes before reaching here.
        (None, None) => "No Environment found.".to_string(),
    }
}

/// Render `n` in lowercase base 36 — the alphabet an `EnvironmentId` body uses.
fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 digits are ASCII")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState>)
}
