// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice service.
//!
//! This is the single store every handler in `src/ops/` reads and writes: the
//! applications, the application versions, the configuration templates, the
//! custom platform versions and the
//! environments the lifecycle
//! operations create, describe and delete, plus the managed platform branch
//! catalog the service publishes and the event log those operations append to
//! and `DescribeEvents` reports. It also owns the few derivations all
//! of them share — the server clock, the request's region, this service's ARN
//! forms, and the clock-owed environment status transitions — so no handler
//! spells one of them a second way.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::ops::types::{
    ApplicationDescription, ApplicationResourceLifecycleConfig, ApplicationVersionDescription,
    ApplicationVersionLifecycleConfig, ConfigurationOptionSetting, CustomAmi, EnvironmentTier,
    EventDescription, ManagedActionHistoryItem, MaxAgeRule, MaxCountRule, OptionSpecification,
    PlatformBranchSummary, PlatformDescription, PlatformFramework, PlatformProgrammingLanguage,
    PlatformSummary, S3Location, Tag,
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
/// AbortEnvironmentUpdate, `Creating` -> `Ready` after CreatePlatformVersion.
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
/// See [`STATUS_LAUNCHING`]. "Updating: Environment is in the process of
/// updating its configuration settings or application version" — the status an
/// environment reports while the update `UpdateEnvironment` starts is in
/// flight.
pub const STATUS_UPDATING: &str = "Updating";

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

/// `PlatformStatus` values this service writes (modelled set:
/// `crate::ops::types::PLATFORM_STATUS_VALUES`). A custom platform version is
/// `Creating` while the version is being built and `Ready` once it can be used.
pub const PLATFORM_STATUS_CREATING: &str = "Creating";
/// See [`PLATFORM_STATUS_CREATING`].
pub const PLATFORM_STATUS_READY: &str = "Ready";

/// The `PlatformOwner` a MANAGED platform version reports: Elastic Beanstalk
/// itself publishes those, so they are owned by the service rather than by an
/// account (a CUSTOM version reports the account id that created it).
pub const MANAGED_PLATFORM_OWNER: &str = "AWSElasticBeanstalk";

/// The `Maintainer` a MANAGED platform version reports ("Information about the
/// maintainer of the platform version"): the address Elastic Beanstalk publishes
/// as the maintainer of the platforms IT builds, which every recorded
/// `DescribePlatformVersion` response for an AWS-managed version carries. A
/// CUSTOM version's maintainer is stated by its own definition archive, which
/// this service does not read, so none is reported for one.
pub const MANAGED_PLATFORM_MAINTAINER: &str = "aws-elasticbeanstalk-platforms@amazon.com";

/// The `SupportedAddon` values this service knows, from
/// `PlatformFilter$Values` ("`SupportedAddon`: `Log/S3` | `Monitoring/Healthd` |
/// `WorkerDaemon/SQSD`" — a list the model introduces as "valid filter values
/// for SOME filter attributes", so it is not closed) plus `Log/CloudWatch`, the
/// CloudWatch Logs streaming add-on the recorded platform-version responses
/// carry beside those three. Sorted, which is also the order a version reports
/// them in.
pub const SUPPORTED_ADDONS: &[&str] = &[
    ADDON_LOG_CLOUDWATCH,
    ADDON_LOG_S3,
    ADDON_MONITORING_HEALTHD,
    ADDON_WORKER_DAEMON_SQSD,
];

/// See [`SUPPORTED_ADDONS`].
pub const ADDON_LOG_CLOUDWATCH: &str = "Log/CloudWatch";
/// See [`SUPPORTED_ADDONS`].
pub const ADDON_LOG_S3: &str = "Log/S3";
/// See [`SUPPORTED_ADDONS`].
pub const ADDON_MONITORING_HEALTHD: &str = "Monitoring/Healthd";
/// See [`SUPPORTED_ADDONS`].
pub const ADDON_WORKER_DAEMON_SQSD: &str = "WorkerDaemon/SQSD";

/// The `CustomAmi$VirtualizationType` values a managed platform version
/// publishes machine images for: the two EC2 virtualization types, in the order
/// the recorded `DescribePlatformVersion` responses list them. Elastic Beanstalk
/// publishes one AMI per virtualization type for a managed platform version, so
/// this is what makes up its `CustomAmiList`.
pub const PLATFORM_VIRTUALIZATION_TYPES: &[&str] = &["pv", "hvm"];

/// How many characters a published AMI's id carries after `ami-`: EC2 issues a
/// machine-image id as `ami-` plus 17 lowercase hexadecimal characters (its long
/// id form). See [`managed_custom_ami_catalog`].
const CUSTOM_AMI_ID_BODY_LENGTH: usize = 17;

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
///
/// The modelled enum spells its values in mixed case (`Unprocessed`), but the
/// real service puts an application version's processing status on the wire
/// UPPER-CASED (`UNPROCESSED`) — what the recorded responses of
/// `CreateApplicationVersion`, `UpdateApplicationVersion` and
/// `DescribeApplicationVersions` all carry — so a client comparing `Status` to
/// what AWS answers reads the same text here. [`version_status_on_the_wire`] is
/// that rendering, applied to whichever modelled value a record holds.
pub const VERSION_STATUS_UNPROCESSED: &str = "Unprocessed";

/// An `ApplicationVersionStatus` as this service puts it on the wire: the
/// modelled value upper-cased. See [`VERSION_STATUS_UNPROCESSED`].
pub fn version_status_on_the_wire(status: &str) -> String {
    status.to_ascii_uppercase()
}

/// The S3 location of the sample application an application version falls back
/// to, in the region the request was signed for.
///
/// From `CreateApplicationVersion$SourceBundle`: "Specify a source bundle in
/// Amazon S3 or a commit in an CodeCommit repository (with
/// `SourceBuildInformation`), but not both. If neither `SourceBundle` nor
/// `SourceBuildInformation` are provided, Elastic Beanstalk uses a sample
/// application." The bucket is the regional Elastic Beanstalk bucket and the key
/// is the sample application's object, as the recorded responses report them, so
/// a version created without a bundle still tells its caller where its source
/// was recorded.
pub fn sample_application_source_bundle(region: &str) -> S3Location {
    S3Location {
        s3_bucket: Some(format!("{SAMPLE_APPLICATION_S3_BUCKET_PREFIX}-{region}")),
        s3_key: Some(SAMPLE_APPLICATION_S3_KEY.to_string()),
    }
}

/// The bucket the sample application's source bundle is reported in, before the
/// region: Elastic Beanstalk's own regional bucket. See
/// [`sample_application_source_bundle`].
pub const SAMPLE_APPLICATION_S3_BUCKET_PREFIX: &str = "elasticbeanstalk";

/// The S3 key of the sample application's source bundle. See
/// [`sample_application_source_bundle`].
pub const SAMPLE_APPLICATION_S3_KEY: &str = "GenericSampleApplication";

/// The version-lifecycle defaults an application carries when its caller
/// specified none: both rules present and DISABLED, keeping a version's source
/// bundle in S3, with the service's published retention figures — 180 days and
/// 200 versions (Elastic Beanstalk's application version lifecycle defaults, as
/// the recorded `CreateApplication` / `DescribeApplications` responses report
/// them). The model gives neither figure, so they are named here once and every
/// path that fills a rule in reads them.
pub const DEFAULT_MAX_AGE_IN_DAYS: i32 = 180;
/// See [`DEFAULT_MAX_AGE_IN_DAYS`].
pub const DEFAULT_MAX_COUNT: i32 = 200;

/// The `MaxAgeRule` an application reports when the request specified none.
pub fn default_max_age_rule() -> MaxAgeRule {
    MaxAgeRule {
        enabled: false,
        max_age_in_days: Some(DEFAULT_MAX_AGE_IN_DAYS),
        delete_source_from_s3: Some(false),
    }
}

/// The `MaxCountRule` an application reports when the request specified none.
pub fn default_max_count_rule() -> MaxCountRule {
    MaxCountRule {
        enabled: false,
        max_count: Some(DEFAULT_MAX_COUNT),
        delete_source_from_s3: Some(false),
    }
}

/// `requested` with every version-lifecycle setting it leaves out filled in from
/// the service defaults.
///
/// An application ALWAYS has a resource lifecycle configuration: the service
/// applies its own version-lifecycle policy to an application whose caller named
/// none, so every read of the application reports both rules and a client can
/// tell what the policy is before changing it. A rule the request omits is the
/// default rule; a setting it omits inside a rule it did supply is that rule's
/// default value. `ServiceRole` is NOT defaulted: it is the caller's own IAM
/// role, and an application that was never given one has none.
pub fn resource_lifecycle_config_with_defaults(
    requested: Option<&ApplicationResourceLifecycleConfig>,
) -> ApplicationResourceLifecycleConfig {
    let requested_version_lifecycle = requested.and_then(|config| config.version_lifecycle_config.as_ref());
    let max_age_rule = match requested_version_lifecycle.and_then(|config| config.max_age_rule.as_ref()) {
        Some(rule) => MaxAgeRule {
            enabled: rule.enabled,
            max_age_in_days: rule.max_age_in_days.or(Some(DEFAULT_MAX_AGE_IN_DAYS)),
            delete_source_from_s3: rule.delete_source_from_s3.or(Some(false)),
        },
        None => default_max_age_rule(),
    };
    let max_count_rule = match requested_version_lifecycle.and_then(|config| config.max_count_rule.as_ref()) {
        Some(rule) => MaxCountRule {
            enabled: rule.enabled,
            max_count: rule.max_count.or(Some(DEFAULT_MAX_COUNT)),
            delete_source_from_s3: rule.delete_source_from_s3.or(Some(false)),
        },
        None => default_max_count_rule(),
    };
    ApplicationResourceLifecycleConfig {
        service_role: requested.and_then(|config| config.service_role.clone()),
        version_lifecycle_config: Some(ApplicationVersionLifecycleConfig {
            max_age_rule: Some(max_age_rule),
            max_count_rule: Some(max_count_rule),
        }),
    }
}

/// The environment tier an environment reports, from the tier the request named.
///
/// "The default is `WebServer`" / "For Standard-mode EC2-based web server —
/// `WebServer`" with type "`Standard`", and `EnvironmentTier$Version`: "The
/// version of this environment tier. When you don't set a value to it, Elastic
/// Beanstalk uses the latest compatible worker tier version" — which is
/// [`DEFAULT_TIER_VERSION`] here, the only tier version this service publishes.
/// So a member the request leaves out is filled in rather than reported absent,
/// and a member it supplies is kept exactly as sent.
pub fn environment_tier_with_defaults(requested: Option<&EnvironmentTier>) -> EnvironmentTier {
    EnvironmentTier {
        name: requested
            .and_then(|tier| tier.name.clone())
            .or_else(|| Some(TIER_NAME_WEB_SERVER.to_string())),
        r#type: requested
            .and_then(|tier| tier.r#type.clone())
            .or_else(|| Some(TIER_TYPE_STANDARD.to_string())),
        version: requested
            .and_then(|tier| tier.version.clone())
            .or_else(|| Some(DEFAULT_TIER_VERSION.to_string())),
    }
}

/// The default `EnvironmentTier$Name`, from its own documentation ("For
/// Standard-mode EC2-based web server — `WebServer`").
pub const TIER_NAME_WEB_SERVER: &str = "WebServer";
/// The `EnvironmentTier$Name` of the WORKER tier, from the same documentation
/// ("For Standard-mode EC2-based backend application with Amazon SQS —
/// `Worker`"): the tier whose environment takes its work from an Amazon SQS
/// queue, which is what gives it a queue among its resources.
pub const TIER_NAME_WORKER: &str = "Worker";
/// The default `EnvironmentTier$Type`, from its own documentation ("For Web
/// server tier — `Standard`").
pub const TIER_TYPE_STANDARD: &str = "Standard";
/// The `EnvironmentTier$Version` this service publishes, which is what "the
/// latest compatible worker tier version" resolves to here and what the recorded
/// `CreateEnvironment` responses report.
pub const DEFAULT_TIER_VERSION: &str = "1.0";

/// `EventSeverity` from least to most severe, in the order the model declares the
/// enum's members (`TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`, `FATAL`) — the
/// alphabetical `crate::ops::types::EVENT_SEVERITY_VALUES` is the same SET in a
/// different order. `DescribeEvents$Severity` is documented as "limits the events
/// returned from this call to include only those with the specified severity or
/// higher", so this order is what "higher" means.
pub const EVENT_SEVERITY_LADDER: &[&str] = &["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"];

/// The severity every event this service records carries: each one reports a
/// lifecycle call that succeeded, which is informational. Nothing here runs a
/// deployment, a health check or a managed action, so there is no failure or
/// degradation to report at a higher severity and none is invented.
pub const SEVERITY_INFO: &str = "INFO";

/// How far back `DescribeEvents` reports: "Returns list of event descriptions
/// matching criteria **up to the last 6 weeks**" (the model's `DescribeEvents`
/// doc). An event older than this is outside the window the operation reports, so
/// the store drops it.
pub const EVENT_RETENTION: Duration = Duration::from_secs(6 * 7 * 24 * 60 * 60);

/// The page size `DescribeEvents` uses when the caller names none: "This action
/// returns the most recent 1,000 events from the specified NextToken" (the
/// model's `DescribeEvents` doc), which is also `MaxRecords`' modelled maximum.
pub const DEFAULT_EVENT_PAGE_SIZE: usize = 1_000;

/// How many events the store keeps.
///
/// [`EVENT_RETENTION`] bounds the log by AGE, which bounds nothing inside one
/// emulator run: a create/terminate loop can append thousands of events in a
/// second, none of them six weeks old. So the log is also capped by COUNT, with
/// the oldest records dropped first. The count is chosen, not documented.
pub const MAX_RETAINED_EVENTS: usize = 10_000;

/// The page size `DescribeEnvironmentManagedActionHistory` answers with when the
/// request names no `MaxItems`: the modelled maximum of that member
/// (`ManagedActionHistoryMaxItems`, `range 1..100`), which is also the maximum
/// length of the `ManagedActionHistoryItems` list one answer can carry
/// (`len 1..100`), so an unbounded request is answered with as large a page as
/// the model admits.
pub const DEFAULT_MANAGED_ACTION_HISTORY_PAGE_SIZE: usize = 100;

/// Where `severity` sits on [`EVENT_SEVERITY_LADDER`], or `None` when it is not
/// one of the modelled values.
pub fn severity_rank(severity: &str) -> Option<usize> {
    EVENT_SEVERITY_LADDER
        .iter()
        .position(|known| *known == severity)
}

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

/// See [`MAX_APPLICATIONS`]: the same published table gives 300 configuration
/// templates (saved configurations) per account per region. Reported by
/// DescribeAccountAttributes as `ConfigurationTemplateQuota` and enforced by
/// `CreateConfigurationTemplate`, which is also what bounds
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::configuration_templates`].
pub const MAX_CONFIGURATION_TEMPLATES: usize = 300;

/// How many custom platform versions one account may hold here.
///
/// Elastic Beanstalk limits custom platform versions per account per region, but
/// the figure is in neither the model nor this service's own documentation, so
/// this bound is CHOSEN. It is what keeps the platform store bounded, and what
/// makes the modelled `TooManyPlatformsException` reachable. It is also what
/// DescribeAccountAttributes reports as `CustomPlatformQuota`, so the quota a
/// client reads is the one `CreatePlatformVersion` refuses a create past.
pub const MAX_PLATFORM_VERSIONS: usize = 50;

/// How many tags one Elastic Beanstalk resource can carry, from the service's
/// published quota ("Tags per resource: 50" in the Elastic Beanstalk service
/// quotas / "Tagging Application Resources" guide). This is the bound
/// `TooManyTagsException` names: "The number of tags in the resource would exceed
/// the number of tags that each resource can have. To calculate this, the
/// operation considers both the number of tags the resource already has and the
/// tags this operation would add if it succeeded."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many option settings ONE configuration — a template or an environment's
/// own set — may carry here.
///
/// Elastic Beanstalk publishes no quota for it, so this bound is CHOSEN: it is
/// comfortably above the size of the service's whole published option catalogue
/// (a few hundred options across all namespaces, which is the most a complete
/// configuration can name once, since an option is identified by
/// [`option_setting_key`] and so appears at most once in a configuration). It is
/// what bounds the SIZE of one stored template — the template COUNT is bounded
/// by [`MAX_CONFIGURATION_TEMPLATES`] — and what bounds the work
/// [`merge_option_settings`] does for one request, which is why the refusal is
/// made on the request's own list BEFORE the merge and on the merged result
/// after it.
pub const MAX_CONFIGURATION_OPTION_SETTINGS: usize = 500;

/// The message a request carrying more than [`MAX_CONFIGURATION_OPTION_SETTINGS`]
/// option settings is refused with, built from the request: the member, the count
/// it sent and the bound it exceeded.
pub fn too_many_option_settings_message(count: usize) -> String {
    format!(
        "Parameter OptionSettings carries {count} option settings. A configuration can hold \
         at most {MAX_CONFIGURATION_OPTION_SETTINGS}."
    )
}

/// How many storage locations this account may hold — the bound
/// `CreateStorageLocation`'s modelled `TooManyBucketsException` names ("The
/// specified account has reached its limit of Amazon S3 buckets"), from Amazon
/// S3's own default quota of 100 general-purpose buckets per account. One
/// bucket per region, so this is also what bounds
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::storage_buckets`]:
/// the region a bucket's name is built from comes from the request, so the
/// number of names a caller can mint is capped like every other store here.
pub const MAX_STORAGE_LOCATIONS: usize = 100;

/// How many terminated environments the store keeps, so DescribeEnvironments can
/// still report them for `IncludeDeleted`.
///
/// A terminated environment no longer counts against [`MAX_ENVIRONMENTS`], so
/// without a retention bound a create/terminate loop would grow the store
/// forever. The bound is a COUNT, not a duration, so the store's size is capped
/// outright; the count is chosen, not documented. The most recently terminated
/// records are the ones kept.
pub const MAX_RETAINED_TERMINATED_ENVIRONMENTS: usize = 200;

/// One solution stack this service offers: the platform BRANCH it is a version
/// within, that version, and the source-bundle file types it permits.
///
/// A solution stack is a concrete platform version inside a published platform
/// branch, so a stack is identified by the branch it belongs to rather than by
/// a second spelling of the runtime: that is what keeps this catalog and the
/// branch catalog [`MANAGED_PLATFORM_BRANCHES`] (what `ListPlatformBranches`
/// reports) from disagreeing about which runtimes the service offers.
#[derive(Debug, Clone, Copy)]
pub struct SolutionStack {
    /// The `BranchName` of the [`MANAGED_PLATFORM_BRANCHES`] entry this stack is
    /// a version within, e.g. `Python 3.13 running on 64bit Amazon Linux 2023`.
    pub branch_name: &'static str,
    /// The platform version: the `v<major>.<minor>.<patch>` component of the
    /// reported `SolutionStackName`.
    pub platform_version: &'static str,
    pub permitted_file_types: &'static [&'static str],
    /// The `PlatformCategory` of every version in this branch — "the category of
    /// platform version", the platform FAMILY the runtime belongs to
    /// (`Python`, `Docker`, `Tomcat`, `.NET Core`). It is catalog data, not a
    /// spelling of the branch name: Elastic Beanstalk publishes the .NET-on-Linux
    /// family as the category `.NET Core` under the platform name `.NET Core on
    /// Linux`, so the two cannot be derived from each other.
    pub platform_category: &'static str,
    /// The `ProgrammingLanguages` this version runs, as `(Name, Version)` pairs.
    /// Catalog data for the same reason as [`Self::platform_category`]: the
    /// branch name states the runtime's marketing name (`Corretto 21`), not the
    /// language and version a client filters and displays. A platform that runs
    /// no language of its own (Docker runs the image's) states none.
    pub programming_languages: &'static [(&'static str, &'static str)],
    /// The `Frameworks` this version runs, as `(Name, Version)` pairs — the
    /// application server or framework the platform ships (a Tomcat platform's
    /// `Tomcat`). Most platforms ship none.
    pub frameworks: &'static [(&'static str, &'static str)],
}

impl SolutionStack {
    /// The `SolutionStackName` this stack is reported under.
    ///
    /// AWS publishes a branch as `<runtime> running on <os>` and a solution
    /// stack within it as `<os> v<version> running <runtime>` — the Python 3.13
    /// branch's current version is `64bit Amazon Linux 2023 v4.6.0 running
    /// Python 3.13` — so the name is DERIVED from the branch rather than
    /// written out a second time and left to drift from it.
    pub fn name(&self) -> String {
        match self.branch_name.split_once(" running on ") {
            Some((runtime, os)) => format!("{os} v{} running {runtime}", self.platform_version),
            // A branch name not in the published form is reported as it stands
            // rather than rewritten into a shape it does not have.
            None => self.branch_name.to_string(),
        }
    }

    /// The `PlatformArn` this managed platform version is named by:
    /// `arn:aws:elasticbeanstalk:<region>::platform/<branch name>/<version>`.
    ///
    /// A managed platform version belongs to the SERVICE, not to an account, so
    /// its ARN carries no account id — which is why it is not built with
    /// [`::engine::emulator_arn`], the form every account-owned resource of this
    /// service uses.
    pub fn platform_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:{ARN_NAMESPACE}:{region}::platform/{}/{}",
            self.branch_name, self.platform_version
        )
    }

    /// How this version is keyed in the catalog's per-version tables (its
    /// published machine images): the resource part of its `PlatformArn`, which
    /// is `<branch name>/<version>` and so names one catalog row in every
    /// region.
    pub fn catalog_key(&self) -> String {
        format!("{}/{}", self.branch_name, self.platform_version)
    }

    /// The runtime and the operating system this stack's branch name is built
    /// from: `<runtime> running on <os>`, e.g. `("Python 3.13", "64bit Amazon
    /// Linux 2023")`. A branch name not in the published form carries no
    /// operating system, so nothing is read out of it.
    fn runtime_and_operating_system(&self) -> Option<(&'static str, &'static str)> {
        self.branch_name.split_once(" running on ")
    }

    /// The `Description` of this published version: "`<os> running <runtime>`" —
    /// its [`Self::name`] without the version component, which is how Elastic
    /// Beanstalk words a managed platform version's description (`64bit Amazon
    /// Linux 2023 running Python 3.13`). DERIVED from the branch, like the
    /// solution stack name, so the two cannot drift apart.
    pub fn description(&self) -> String {
        match self.runtime_and_operating_system() {
            Some((runtime, os)) => format!("{os} running {runtime}"),
            None => self.branch_name.to_string(),
        }
    }

    /// The `OperatingSystemName` and `OperatingSystemVersion` of this version.
    ///
    /// Elastic Beanstalk names a branch's operating system inside the branch
    /// name, as `64bit <name> <version>` (`64bit Amazon Linux 2023`, `64bit
    /// Windows Server 2019`), so both members are READ OFF the branch rather
    /// than written out a second time: the width prefix is dropped, a trailing
    /// numeric component is the version, and what precedes it is the name. An
    /// operating system named without a version component (`64bit Amazon Linux`)
    /// reports no version.
    pub fn operating_system(&self) -> Option<(&'static str, Option<&'static str>)> {
        let (_, os) = self.runtime_and_operating_system()?;
        let os = os.strip_prefix("64bit ").unwrap_or(os);
        match os.rsplit_once(' ') {
            Some((name, version))
                if !version.is_empty()
                    && version
                        .chars()
                        .all(|character| character.is_ascii_digit() || character == '.') =>
            {
                Some((name, Some(version)))
            }
            _ => Some((os, None)),
        }
    }

    /// The modelled `ProgrammingLanguages` of this published version. A catalog
    /// entry that states no version for its language reports the language
    /// without one rather than an empty version string.
    pub fn to_programming_languages(&self) -> Vec<PlatformProgrammingLanguage> {
        self.programming_languages
            .iter()
            .map(|(name, version)| PlatformProgrammingLanguage {
                name: Some((*name).to_string()),
                version: stated(version),
            })
            .collect()
    }

    /// The names of the programming languages this version runs — the attribute
    /// a `ListPlatformVersions` filter term of type `ProgrammingLanguageName`
    /// is applied to.
    pub fn programming_language_names(&self) -> Vec<String> {
        self.programming_languages
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect()
    }

    /// The modelled `Frameworks` of this published version. As in
    /// [`Self::to_programming_languages`], a version the catalog does not state
    /// is reported absent.
    pub fn to_frameworks(&self) -> Vec<PlatformFramework> {
        self.frameworks
            .iter()
            .map(|(name, version)| PlatformFramework {
                name: Some((*name).to_string()),
                version: stated(version),
            })
            .collect()
    }

    /// The modelled `PlatformDescription` for this published version.
    ///
    /// Everything the catalog states about the version is projected here, and
    /// everything the STORE holds about it comes from `store`: the
    /// [`MANAGED_PLATFORM_BRANCHES`] entry it is a version within — the one place
    /// its supported tiers and its branch life-cycle state are held, so
    /// `ListPlatformBranches` and this call cannot disagree about them — the
    /// machine images the catalog published for it, and when the catalog was
    /// published. A stack whose branch the store does not hold reports neither
    /// branch member rather than a guessed one.
    pub fn to_description(
        &self,
        region: &str,
        store: &ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    ) -> PlatformDescription {
        let operating_system = self.operating_system();
        let branch = store.platform_branch_named(self.branch_name);
        PlatformDescription {
            platform_arn: Some(self.platform_arn(region)),
            platform_owner: Some(MANAGED_PLATFORM_OWNER.to_string()),
            platform_name: Some(self.branch_name.to_string()),
            platform_version: Some(self.platform_version.to_string()),
            platform_branch_name: Some(self.branch_name.to_string()),
            solution_stack_name: Some(self.name()),
            // A published version is usable as it stands; nothing is building it.
            platform_status: Some(PLATFORM_STATUS_READY.to_string()),
            // The catalog is service-owned reference data: it was published when
            // this store was created and has not been re-published since.
            date_created: Some(store.platform_catalog_published_at),
            date_updated: Some(store.platform_catalog_published_at),
            platform_category: Some(self.platform_category.to_string()),
            description: Some(self.description()),
            maintainer: Some(MANAGED_PLATFORM_MAINTAINER.to_string()),
            operating_system_name: operating_system.map(|(name, _)| name.to_string()),
            operating_system_version: operating_system
                .and_then(|(_, version)| version)
                .map(str::to_string),
            programming_languages: Some(self.to_programming_languages()),
            frameworks: Some(self.to_frameworks()),
            custom_ami_list: Some(
                store
                    .platform_custom_amis
                    .get(&self.catalog_key())
                    .cloned()
                    .unwrap_or_default(),
            ),
            supported_tier_list: branch.map(|branch| branch.supported_tiers.clone()),
            // Both lists follow the BRANCH, so a stack whose branch the store
            // does not hold reports neither rather than one of the two.
            supported_addon_list: branch
                .map(|branch| managed_supported_addons(&branch.supported_tiers)),
            // The catalog publishes exactly one version per branch, so every
            // published version IS its branch's recommended one.
            platform_lifecycle_state: Some(PLATFORM_LIFECYCLE_RECOMMENDED.to_string()),
            platform_branch_lifecycle_state: branch
                .map(|branch| reported_branch_lifecycle_state(&branch.lifecycle_state)),
        }
    }
}

/// A catalog value as a modelled member: the value itself, or no value at all
/// when the catalog states none (the empty string).
fn stated(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// The `CustomAmiList` the catalog publishes for every managed platform
/// version, by [`SolutionStack::catalog_key`]: one machine image per
/// [`PLATFORM_VIRTUALIZATION_TYPES`] value, which is what
/// `DescribePlatformVersion` reports as "the custom AMIs supported by the
/// platform version".
///
/// The image IDS are SERVER-GENERATED, exactly like an `EnvironmentId`: an
/// `ImageId` is opaque ("THe ID of the image used to create the custom AMI" and
/// nothing else), so nothing is derived from a request or from a recording — the
/// body is random, in the `ami-` long-id form EC2 issues, drawn through the same
/// uuid-backed helper every other id of this service comes from. They are minted
/// ONCE, when the store is created, so every read of a platform version reports
/// the same images and no read mints one.
fn managed_custom_ami_catalog() -> BTreeMap<String, Vec<CustomAmi>> {
    SOLUTION_STACKS
        .iter()
        .map(|stack| {
            let images = PLATFORM_VIRTUALIZATION_TYPES
                .iter()
                .map(|virtualization_type| CustomAmi {
                    virtualization_type: Some((*virtualization_type).to_string()),
                    image_id: Some(format!("ami-{}", random_id_body(CUSTOM_AMI_ID_BODY_LENGTH))),
                })
                .collect();
            (stack.catalog_key(), images)
        })
        .collect()
}

/// The `SupportedAddonList` of a managed platform version whose branch supports
/// `supported_tiers` — "the additions associated with the platform version".
///
/// Every platform version Elastic Beanstalk publishes streams its logs to Amazon
/// S3 and to CloudWatch Logs and reports enhanced health through the health
/// daemon; the SQS daemon is what runs a WORKER environment, so it is an
/// addition only of a version whose branch supports the worker tier. That is the
/// rule, so the list follows the branch's tiers rather than being written out
/// per catalog entry.
pub fn managed_supported_addons(supported_tiers: &[String]) -> Vec<String> {
    SUPPORTED_ADDONS
        .iter()
        .filter(|addon| {
            **addon != ADDON_WORKER_DAEMON_SQSD
                || supported_tiers.iter().any(|tier| tier == TIER_WORKER)
        })
        .map(|addon| (*addon).to_string())
        .collect()
}

/// A platform branch's life-cycle state as a PLATFORM VERSION reports it
/// (`PlatformSummary$PlatformBranchLifecycleState`,
/// `PlatformDescription$PlatformBranchLifecycleState`): the branch catalog's own
/// state, capitalized.
///
/// The model spells this value set twice and differently — `PlatformSummary`
/// documents "`beta` | `supported` | `deprecated` | `retired`" while
/// `PlatformDescription` documents "`Beta` | `Supported` | `Deprecated` |
/// `Retired`" — and both cannot describe one attribute. The recorded
/// platform-version responses of the real service settle it: they carry the
/// capitalized form on BOTH shapes, so that is the form a platform version
/// reports. The branch catalog keeps the spelling it holds (`ListPlatformBranches`
/// reports its own member from it), so this rendering is IDEMPOTENT: a state
/// already capitalized comes back unchanged.
pub fn reported_branch_lifecycle_state(state: &str) -> String {
    let mut characters = state.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The published managed platform version `platform_arn` names in `region`, if
/// the catalog publishes one.
///
/// This is the other half of resolving a `PlatformArn`: a CUSTOM version lives in
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::platform_versions`],
/// a MANAGED one in [`SOLUTION_STACKS`], and an ARN that answers to neither names
/// no platform version this service can launch.
pub fn managed_platform_version(region: &str, platform_arn: &str) -> Option<&'static SolutionStack> {
    SOLUTION_STACKS
        .iter()
        .find(|stack| stack.platform_arn(region) == platform_arn)
}

/// The published managed platform version reported under the
/// `SolutionStackName` `name`, if the catalog publishes one.
///
/// A solution stack name and a platform version ARN name the SAME platform
/// version from the one catalog ([`SOLUTION_STACKS`]), so an operation handed
/// either one resolves the other through here rather than reporting only the
/// form the caller happened to use.
pub fn solution_stack_named(name: &str) -> Option<&'static SolutionStack> {
    SOLUTION_STACKS.iter().find(|stack| stack.name() == name)
}

/// The solution stacks this emulator offers, which is what
/// `ListAvailableSolutionStacks` reports: the current version of every platform
/// branch in [`MANAGED_PLATFORM_BRANCHES`] that has not retired, because a
/// retired branch has no version an account can still deploy on.
///
/// This is the SERVICE's own platform catalog, not anything a caller creates,
/// so it is a table here rather than a map in
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState`]. Each entry
/// names its branch, so the two catalogs agree by construction; the version
/// components are this emulator's own, because AWS re-publishes a platform
/// version with every platform release and no emulator can track those.
/// `PermittedFileTypes` is "the permitted file types allowed for a solution
/// stack": the archive that runtime deploys from — a Tomcat platform deploys a
/// `war`, a Java SE (Corretto) platform a `jar`, and every platform accepts a
/// `zip` source bundle.
///
/// `platform_category`, `programming_languages` and `frameworks` are the
/// platform FAMILY, the language runtime and the application server Elastic
/// Beanstalk publishes for each of its platforms (the "Elastic Beanstalk
/// supported platforms" document: the Python platforms run the application
/// behind Gunicorn, the Ruby platforms behind Puma, the PHP platforms behind
/// PHP-FPM, a Tomcat platform on Tomcat and a Windows platform on IIS), carried
/// here because none of the three can be derived from the branch name: the
/// .NET-on-Linux family's category is `.NET Core`, a Corretto branch runs the
/// language `Java`, and a Tomcat branch runs Tomcat ON Java. A platform that
/// runs no language of its own (Docker runs whatever the image holds) states
/// none, and a runtime or framework whose version the catalog does not state is
/// reported without one.
///
/// ORDER IS CONTRACTUAL: the operation reports the stacks "with the public
/// version first and then in reverse chronological order", and the handler
/// reports this table as it stands, so the table is written newest branch first
/// within each platform, in [`MANAGED_PLATFORM_BRANCHES`]' own order.
pub const SOLUTION_STACKS: &[SolutionStack] = &[
    // .NET on Linux.
    SolutionStack {
        branch_name: ".NET 9 running on 64bit Amazon Linux 2023",
        platform_version: "3.4.0",
        permitted_file_types: &["zip"],
        platform_category: ".NET Core",
        programming_languages: &[(".NET", "9")],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: ".NET 8 running on 64bit Amazon Linux 2023",
        platform_version: "3.3.0",
        permitted_file_types: &["zip"],
        platform_category: ".NET Core",
        programming_languages: &[(".NET", "8")],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: ".NET Core running on 64bit Amazon Linux 2",
        platform_version: "2.8.0",
        permitted_file_types: &["zip"],
        platform_category: ".NET Core",
        programming_languages: &[(".NET Core", "")],
        frameworks: &[],
    },
    // .NET on Windows Server.
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2019",
        platform_version: "2.16.0",
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
        frameworks: &[("IIS", "10.0")],
    },
    // Docker.
    SolutionStack {
        branch_name: "Docker running on 64bit Amazon Linux 2023",
        platform_version: "4.2.0",
        permitted_file_types: &["zip"],
        platform_category: "Docker",
        programming_languages: &[],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: "Docker running on 64bit Amazon Linux 2",
        platform_version: "3.9.0",
        permitted_file_types: &["zip"],
        platform_category: "Docker",
        programming_languages: &[],
        frameworks: &[],
    },
    // Go.
    SolutionStack {
        branch_name: "Go 1 running on 64bit Amazon Linux 2023",
        platform_version: "3.5.0",
        permitted_file_types: &["zip"],
        platform_category: "Go",
        programming_languages: &[("Go", "1")],
        frameworks: &[],
    },
    // Java SE (Corretto): a Java SE platform deploys a `jar`.
    SolutionStack {
        branch_name: "Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: "4.4.0",
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "21")],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: "Corretto 17 running on 64bit Amazon Linux 2023",
        platform_version: "4.3.0",
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "17")],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: "Corretto 11 running on 64bit Amazon Linux 2",
        platform_version: "3.7.0",
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "11")],
        frameworks: &[],
    },
    // Node.js.
    SolutionStack {
        branch_name: "Node.js 22 running on 64bit Amazon Linux 2023",
        platform_version: "6.6.0",
        permitted_file_types: &["zip"],
        platform_category: "Node.js",
        programming_languages: &[("Node.js", "22")],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: "Node.js 20 running on 64bit Amazon Linux 2023",
        platform_version: "6.5.0",
        permitted_file_types: &["zip"],
        platform_category: "Node.js",
        programming_languages: &[("Node.js", "20")],
        frameworks: &[],
    },
    SolutionStack {
        branch_name: "Node.js 18 running on 64bit Amazon Linux 2023",
        platform_version: "6.4.0",
        permitted_file_types: &["zip"],
        platform_category: "Node.js",
        programming_languages: &[("Node.js", "18")],
        frameworks: &[],
    },
    // PHP.
    SolutionStack {
        branch_name: "PHP 8.4 running on 64bit Amazon Linux 2023",
        platform_version: "4.5.0",
        permitted_file_types: &["zip"],
        platform_category: "PHP",
        programming_languages: &[("PHP", "8.4")],
        frameworks: &[("PHP-FPM", "")],
    },
    SolutionStack {
        branch_name: "PHP 8.3 running on 64bit Amazon Linux 2023",
        platform_version: "4.4.0",
        permitted_file_types: &["zip"],
        platform_category: "PHP",
        programming_languages: &[("PHP", "8.3")],
        frameworks: &[("PHP-FPM", "")],
    },
    // Python.
    SolutionStack {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        platform_version: "4.6.0",
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.13")],
        frameworks: &[("Gunicorn", "")],
    },
    SolutionStack {
        branch_name: "Python 3.11 running on 64bit Amazon Linux 2023",
        platform_version: "4.5.0",
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.11")],
        frameworks: &[("Gunicorn", "")],
    },
    SolutionStack {
        branch_name: "Python 3.9 running on 64bit Amazon Linux 2023",
        platform_version: "4.4.0",
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.9")],
        frameworks: &[("Gunicorn", "")],
    },
    // Ruby.
    SolutionStack {
        branch_name: "Ruby 3.4 running on 64bit Amazon Linux 2023",
        platform_version: "3.5.0",
        permitted_file_types: &["zip"],
        platform_category: "Ruby",
        programming_languages: &[("Ruby", "3.4")],
        frameworks: &[("Puma", "")],
    },
    SolutionStack {
        branch_name: "Ruby 3.2 running on 64bit Amazon Linux 2023",
        platform_version: "3.4.0",
        permitted_file_types: &["zip"],
        platform_category: "Ruby",
        programming_languages: &[("Ruby", "3.2")],
        frameworks: &[("Puma", "")],
    },
    // Tomcat: a Tomcat platform deploys a `war`.
    SolutionStack {
        branch_name: "Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: "5.4.0",
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "21")],
        frameworks: &[("Tomcat", "10.1")],
    },
    SolutionStack {
        branch_name: "Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023",
        platform_version: "5.3.0",
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "11")],
        frameworks: &[("Tomcat", "9")],
    },
];

/// The server clock, as the modelled timestamp type.
///
/// Dated to the MICROSECOND the wire renders: the `date-time` rendering a client
/// reads carries at most microseconds, so a timestamp stored with a finer fraction
/// would be compared against (`DescribeEvents`' `StartTime` / `EndTime` window, the
/// retention horizon) at a value the client was never shown, and a timestamp read
/// back out of a response would not select the record it came from. Building from
/// whole seconds plus truncated subsecond microseconds — rather than from an
/// `f64` of seconds, whose resolution at the current epoch is a fraction of a
/// microsecond — keeps the stored instant and the rendered one the same instant.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_micros() * 1_000,
    )
}

/// One instant as an EXACT ordering key: whole epoch seconds, then the
/// nanoseconds inside that second.
///
/// A comparison that has to tell two instants a microsecond apart apart cannot
/// go through `DateTime::as_secs_f64`: an epoch second count needs ten digits
/// before the point, which leaves an `f64` a step of about a quarter of a
/// microsecond at that magnitude, so two timestamps the service rendered as
/// different (`…:05.334986Z` and `…:05.334987Z`) can compare EQUAL as `f64` —
/// and a `DescribeEvents` window that opens at the later one then reports the
/// earlier one too, in front of a bound its own documentation calls "on or
/// after this time". A lexicographic `(secs, subsec_nanos)` key compares the
/// two integers the timestamp is made of, so it is exact at every epoch.
pub fn instant_key(at: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The region this request is addressed to: the region field of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it carries none.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh id for "the web service request ID for the activity of this event"
/// (`EventDescription$RequestId`): a v4 UUID, the form the real service reports
/// it in.
///
/// The response envelope's own request id is minted inside the protocol
/// serializer, after a handler has returned, so a handler cannot record THAT id.
/// This is the id this service records for the activity of one call: every event
/// a single call appends shares it, and no two calls share one. It is a
/// server-generated per-run value, so nothing is derived from the request.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()`
/// rendered canonically — `vendor/emulator-telemetry/src/types.rs`), which this
/// crate already depends on: nothing here is a hand-rolled generator and this
/// crate declares no second path to the same crate.
pub fn new_request_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The Amazon S3 bucket Elastic Beanstalk stores an account's application
/// versions, logs and other environment files in, in `region`:
/// `elasticbeanstalk-<region>-<account>`, the form `CreateStorageLocation`
/// reports. One bucket per account per region, so the name is DERIVED rather
/// than generated: "If the storage location already exists,
/// `CreateStorageLocation` still returns the bucket name but does not create a
/// new bucket."
pub fn storage_bucket_name(region: &str) -> String {
    format!(
        "{ARN_NAMESPACE}-{region}-{}",
        ::engine::emulator_account_id()
    )
}

/// The Amazon S3 error codes that say the object a read named IS NOT THERE: the
/// bucket does not exist, or the key does not. `NoSuchBucket` and `NoSuchKey` are the
/// codes Amazon S3's own model declares for a read
/// (`smithy_query(query="operation", operation="GetObject")` on the S3 model), and
/// `NotFound` is what a HEAD — which carries no error document to name them in —
/// reports instead. All three are answered with HTTP 404.
const ABSENT_ARCHIVE_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// The HTTP status Amazon S3 answers a read of an object that IS NOT THERE with —
/// an absent bucket and an absent key are both a 404 — and the status it answers a
/// read it does not PERMIT with. These two are what the service can conclude
/// something about the caller's archive from; every other answer says nothing about
/// it (see [`archive_download_refusal`]).
const ARCHIVE_ABSENT_STATUS: u16 = 404;
/// See [`ARCHIVE_ABSENT_STATUS`].
const ARCHIVE_DENIED_STATUS: u16 = 403;

/// The Amazon S3 error codes that say the read of the OBJECT was not PERMITTED, from
/// Amazon S3's published REST error-code list (both are answered with HTTP 403).
///
/// A 403 is only about the archive when it is one of these: a code about the
/// CREDENTIALS the read was signed with (`InvalidAccessKeyId`,
/// `SignatureDoesNotMatch`, and the emulator's own authentication refusal) says
/// nothing about the caller's object, so it falls to the logged branch in
/// [`archive_download_refusal`] rather than telling a client its archive is
/// unreadable.
const DENIED_ARCHIVE_CODES: &[&str] = &["AccessDenied", "AllAccessDisabled"];

/// The reason a refusal carries when the failed read named no HTTP status to take a
/// reason phrase from: an archive the service could not read is, at minimum, not
/// there.
const ARCHIVE_NOT_FOUND_REASON: &str = "Not Found";

/// The message Elastic Beanstalk refuses a create whose Amazon S3 archive it could
/// not download with, built from the request:
///
/// ```text
/// Unable to download from S3 location (Bucket: <bucket>  Key: <key>). Reason: <reason>
/// ```
///
/// Two spaces before `Key:`, as the recorded `CreateApplicationVersion` and
/// `CreatePlatformVersion` refusals carry it. The bucket and the key are the
/// request's own, and the reason is the HTTP reason phrase of the Amazon S3 read
/// that failed.
pub fn unable_to_download_message(bucket: &str, key: &str, reason: &str) -> String {
    format!("Unable to download from S3 location (Bucket: {bucket}  Key: {key}). Reason: {reason}")
}

/// The Amazon S3 client an operation reads a caller-named archive's metadata through,
/// over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and this
/// crate names no other service's crate. `region` is the request's own signing
/// region, so the read happens where the caller's own Amazon S3 requests go.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so the
        // request addresses its bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch
        // for a stalled stream, and each of those would need a sleep implementation
        // this client is not given. The same goes for caching the identity: the
        // credentials are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The refusal message for the Amazon S3 archive at `location`, or `None` when object
/// storage says the archive is there and readable.
///
/// Elastic Beanstalk builds a resource FROM the archive a create names
/// (`CreateApplicationVersion$SourceBundle`, `CreatePlatformVersion$PlatformDefinitionBundle`),
/// so it reads that archive while serving the create and refuses the request when it
/// cannot — which is why a typo'd bucket or key is a 400 at the create rather than a
/// resource a client believes is deployable. The read goes to object storage, in
/// process: nothing here decides from the SHAPE of the bucket or key whether the
/// archive exists.
///
/// It is a `HeadObject`, not a `GetObject`: "a HEAD request has the same options as a
/// GET operation on an object" and answers the same existence and authorization
/// verdict, with NO body — so whether the archive can be read is settled without ever
/// materialising a caller-sized object in this process. Nothing here interprets the
/// archive's contents, so its bytes are never needed. Exactly one such read per
/// request, no loop, nothing retained.
///
/// The read is a read: it changes nothing a later caller sees, in this service's
/// store or in object storage.
///
/// A refusal is only ever grounded in an answer about the OBJECT, which is what the
/// two statuses below are: 404 (the bucket or the key is not there) and 403 carrying
/// one of [`DENIED_ARCHIVE_CODES`]. Anything else says nothing about the caller's
/// archive — an emulator assembled without object storage answers the router's own
/// `UnknownService`, an authentication refusal is about the signer, and a transport
/// failure answers nothing at all — so it is logged and the create proceeds.
pub async fn archive_download_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> Option<String> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let bucket = location.s3_bucket.as_deref().unwrap_or_default();
    let key = location.s3_key.as_deref().unwrap_or_default();
    // A location that names no bucket, or no key, names no Amazon S3 object at all:
    // there is nothing to read and nothing to ask object storage about.
    if bucket.is_empty() || key.is_empty() {
        return Some(unable_to_download_message(
            bucket,
            key,
            ARCHIVE_NOT_FOUND_REASON,
        ));
    }
    let failure = match s3_client(ctx, region)
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
    {
        // The archive is there and readable, so the create goes ahead.
        Ok(_) => return None,
        Err(failure) => failure,
    };
    let status = failure
        .raw_response()
        .map(|response| response.status().as_u16());
    let code = failure.code().unwrap_or_default().to_string();
    let about_the_object = match status {
        Some(ARCHIVE_ABSENT_STATUS) => ABSENT_ARCHIVE_CODES.contains(&code.as_str()),
        Some(ARCHIVE_DENIED_STATUS) => DENIED_ARCHIVE_CODES.contains(&code.as_str()),
        _ => false,
    };
    if !about_the_object {
        ::tracing::warn!(
            bucket,
            key,
            code = %code,
            status = ?status,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
            "the Amazon S3 read of a caller-named archive answered nothing about the object, \
             so the create was not refused for it",
        );
        return None;
    }
    // The reason the refusal carries is the HTTP reason phrase of the read that
    // failed (404 -> `Not Found`, 403 -> `Forbidden`), so it is read off the status
    // Amazon S3 answered rather than written out per error code.
    let reason = status
        .and_then(|status| ::http::StatusCode::from_u16(status).ok())
        .and_then(|status| status.canonical_reason())
        .unwrap_or(ARCHIVE_NOT_FOUND_REASON);
    Some(unable_to_download_message(bucket, key, reason))
}

/// The HTTP status Amazon S3 answers a `CreateBucket` whose name the one bucket
/// namespace already holds with: `BucketAlreadyOwnedByYou` and `BucketAlreadyExists`
/// — the two errors `CreateBucket` models — are both 409.
const BUCKET_EXISTS_STATUS: u16 = 409;

/// The Region an Amazon S3 `CreateBucket` that carries NO location constraint places
/// its bucket in, from `CreateBucketConfiguration$LocationConstraint`'s own
/// documentation: "If you don't specify a Region, the bucket is created in the US East
/// (N. Virginia) Region (us-east-1) by default." It is also why
/// `BucketLocationConstraint` enumerates every other Region code but not this one, so
/// a bucket in this Region is created with no configuration at all.
const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// Amazon S3's published REST error code for an account that has reached its bucket
/// limit — the refusal `CreateStorageLocation`'s modelled `TooManyBucketsException`
/// ("The specified account has reached its limit of Amazon S3 buckets") is about.
const TOO_MANY_BUCKETS_CODE: &str = "TooManyBuckets";

/// Why object storage would not stand the account's Elastic Beanstalk storage bucket
/// up, when its answer said something about the bucket at all. Each variant is the
/// modelled `CreateStorageLocation` error the caller is owed for it; see
/// [`create_storage_bucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBucketRefusal {
    /// The create was not PERMITTED (HTTP 403 carrying one of
    /// [`DENIED_ARCHIVE_CODES`]) — `InsufficientPrivilegesException`.
    NotPermitted,
    /// The account is at its Amazon S3 bucket limit — `TooManyBucketsException`.
    TooManyBuckets,
}

/// Create the account's Elastic Beanstalk storage bucket named `bucket` in `region`
/// in object storage, unless it is already there.
///
/// `CreateStorageLocation` "creates a bucket in Amazon S3 to store application
/// versions, logs, and other files used by Elastic Beanstalk environments", and the
/// bucket it reports is the one a client then uploads its source bundle into — so the
/// bucket has to EXIST in the emulator's object storage, which is Amazon S3's own
/// service, not merely be named in this service's store. The create goes over the
/// in-process callback (see [`s3_client`]), so the bucket a client addresses is the
/// bucket object storage really holds.
///
/// IDEMPOTENT, which is the operation's own contract ("if the storage location already
/// exists, `CreateStorageLocation` still returns the bucket name but does not create a
/// new bucket"): a name the one bucket namespace already holds answers 409, and this
/// emulator serves one account, so that bucket is the account's own storage location
/// and the call reports it.
///
/// A refusal is only ever grounded in an answer about the BUCKET: a 403 carrying one of
/// [`DENIED_ARCHIVE_CODES`] (the permission-refusal codes Amazon S3 answers any request
/// with, read off the same published list the archive read uses) and the bucket-limit
/// code. Anything else says nothing about whether object storage holds the bucket — an
/// emulator assembled without object storage answers the router's own `UnknownService`,
/// an authentication refusal is about the signer, a transport failure answers nothing at
/// all — so it is logged and the storage location is still reported, the way
/// [`archive_download_refusal`] treats the same class of answers.
pub async fn create_storage_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> Result<(), StorageBucketRefusal> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    // One storage location per account per REGION, so the bucket is placed in the
    // request's own region — which on `CreateBucket` is said with the location
    // constraint, not with the signing region. See [`UNCONSTRAINED_BUCKET_REGION`] for
    // the one region that is said by saying nothing.
    let mut create = s3_client(ctx, region).create_bucket().bucket(bucket);
    if region != UNCONSTRAINED_BUCKET_REGION {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    let failure = match create.send().await {
        // The bucket is there now.
        Ok(_) => return Ok(()),
        Err(failure) => failure,
    };
    let status = failure
        .raw_response()
        .map(|response| response.status().as_u16());
    let code = failure.code().unwrap_or_default().to_string();
    match (status, code.as_str()) {
        // The bucket was already there, so there was nothing to create and the
        // storage location is reported as it stands.
        (Some(BUCKET_EXISTS_STATUS), _) => Ok(()),
        (Some(ARCHIVE_DENIED_STATUS), code) if DENIED_ARCHIVE_CODES.contains(&code) => {
            Err(StorageBucketRefusal::NotPermitted)
        }
        // Amazon S3's bucket quota is far above this service's own
        // [`MAX_STORAGE_LOCATIONS`], so this is the account having spent its buckets
        // elsewhere rather than on storage locations.
        (_, TOO_MANY_BUCKETS_CODE) => Err(StorageBucketRefusal::TooManyBuckets),
        _ => {
            ::tracing::warn!(
                bucket,
                region,
                code = %code,
                status = ?status,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the Amazon S3 create of the account's Elastic Beanstalk storage bucket \
                 answered nothing about the bucket, so the storage location was reported \
                 as it stands",
            );
            Ok(())
        }
    }
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

/// `arn:aws:elasticbeanstalk:<region>:<account>:configurationtemplate/<app>/<template>`
/// — the ARN form Elastic Beanstalk names a saved configuration by, which is the
/// `configurationtemplate` member of [`TAGGABLE_RESOURCE_TYPES`] and the form
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::resolve_tagged_resource`]
/// resolves back to a stored template. No modelled response carries a
/// template's ARN, so this is the one statement of that form.
pub fn configuration_template_arn(
    region: &str,
    application_name: &str,
    template_name: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("configurationtemplate/{application_name}/{template_name}"),
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

/// `arn:aws:elasticbeanstalk:<region>:<account>:platform/<name>/<version>` — the
/// ARN of a CUSTOM platform version, which belongs to the account that created
/// it (`CreateEnvironmentMessage$PlatformArn`: "The Amazon Resource Name (ARN) of
/// the custom platform to use with the environment").
pub fn platform_version_arn(region: &str, platform_name: &str, platform_version: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("platform/{platform_name}/{platform_version}"),
    )
}

/// The DNS name an environment is reached at:
/// `<cname prefix>.<region>.elasticbeanstalk.com`.
pub fn environment_cname(cname_prefix: &str, region: &str) -> String {
    format!("{cname_prefix}.{region}.elasticbeanstalk.com")
}

/// The resource portion of `arn` when it is an ARN of THIS service, i.e.
/// `arn:<partition>:elasticbeanstalk:<region>:<account>:<resource>` — the only
/// ARNs `ResourceArn` accepts ("Must be the ARN of an Elastic Beanstalk
/// resource"). The region and account are not matched: this store is not
/// partitioned by either, and the ARN forms above are the only ones it mints.
fn elasticbeanstalk_arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(ARN_NAMESPACE), Some(_region), Some(_account)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    fields.next().filter(|resource| !resource.is_empty())
}

/// The resource types an Elastic Beanstalk ARN can name. Elastic Beanstalk
/// "supports tagging of all of its resources" (`UpdateTagsForResource`), so a
/// type in this set is a resource that CAN carry tags even where this store holds
/// no record of one; a type outside it is what
/// `ResourceTypeNotSupportedException` ("The type of the specified Amazon
/// Resource Name (ARN) isn't supported for this operation") covers.
pub const TAGGABLE_RESOURCE_TYPES: &[&str] = &[
    "application",
    "applicationversion",
    "configurationtemplate",
    "environment",
    "platform",
];

/// Why an operation that takes the `EnvironmentId` / `EnvironmentName` pair
/// resolved no environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentRefusal {
    /// The request named neither identifier. Which code that answers is the
    /// OPERATION's: the model documents `MissingRequiredParameter` for some of
    /// them ("If you do not specify either, Elastic Beanstalk returns
    /// MissingRequiredParameter error") and nothing for the rest, whose recorded
    /// refusal is the generic query `MissingParameter`.
    NoIdentifier,
    /// No environment matched; carries the message naming what was asked for.
    NotFound(String),
}

/// A taggable Elastic Beanstalk resource this store holds: which record a
/// `ResourceArn` resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// An application, by `ApplicationName`.
    Application(String),
    /// An application version, by `(ApplicationName, VersionLabel)`.
    ApplicationVersion(String, String),
    /// A configuration template, by `(ApplicationName, TemplateName)`.
    ConfigurationTemplate(String, String),
    /// An environment, by `EnvironmentId`.
    Environment(String),
}

/// Why a `ResourceArn` resolves to no taggable resource in this store. Each
/// variant is a DIFFERENT refusal, so the two tag operations answer the same
/// three codes for the same three cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnRefusal {
    /// The value is not an ARN of this service at all — another service's
    /// namespace, or not an ARN.
    NotThisService,
    /// An Elastic Beanstalk ARN naming a resource type that is not one of
    /// [`TAGGABLE_RESOURCE_TYPES`].
    UnsupportedType,
    /// A well-formed Elastic Beanstalk ARN of a taggable type whose resource
    /// this store does not hold.
    NotFound,
}

/// Whether `requested` leaves the call with no `ServiceRole` it can be served with:
/// the ONE check both supporting calls make on that member.
///
/// Two requests fail it, and the service reports both the same way — as the member
/// whose value is not an IAM role ARN (see [`invalid_parameter_message`]):
///
/// * one that SUPPLIES a value that is not an IAM role ARN (an empty value included:
///   a value was sent and it is not one);
/// * one that supplies NONE where the call needs one — "The `ServiceRole` property is
///   required the first time that you provide a `VersionLifecycleConfig` for the
///   application in one of the supporting calls (`CreateApplication` or
///   `UpdateApplicationResourceLifecycle`). After you provide it once, in either one of
///   the calls, Elastic Beanstalk persists the Service Role with the application, and
///   you don't need to specify it again in subsequent
///   `UpdateApplicationResourceLifecycle` calls."
///
/// So a request that asks for a version lifecycle configuration is served by the role
/// ALREADY PERSISTED with the application when it supplies none, and refused when
/// neither it nor the application carries one.
pub fn service_role_unusable(
    stored: Option<&ApplicationResourceLifecycleConfig>,
    requested: &ApplicationResourceLifecycleConfig,
) -> bool {
    if let Some(supplied) = requested.service_role.as_ref() {
        return !is_iam_role_arn(supplied);
    }
    requested.version_lifecycle_config.is_some()
        && !stored
            .and_then(|config| config.service_role.as_deref())
            .is_some_and(is_iam_role_arn)
}

/// The form `ApplicationResourceLifecycleConfig$ServiceRole` states: "The ARN of an
/// IAM service role that Elastic Beanstalk has permission to assume."
///
/// An IAM role ARN is `arn:<partition>:iam::<account>:role/<name>`: IAM is a
/// partition-global service, so the Region field is empty, and the resource names the
/// `role` type and a non-empty name (a name may itself carry a path,
/// `role/service-role/<name>`). The account is either empty or the 12 digits an AWS
/// account id has, and the partition is `aws` or `aws-` plus a non-empty suffix. That
/// grammar is the one every AWS model states for an IAM role ARN —
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, e.g.
/// `com.amazonaws.bedrockagentcorecontrol#RoleArn`, and
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$` on Comprehend's `IamRoleArn`.
///
/// The six fields are split here rather than through `services/common`'s shared ARN
/// reader for the reason `tests/all/lifecycle.rs` already records for its own helpers: a
/// new dependency has to be recorded in `Cargo.lock`, the one file this branch shares
/// with every other fix landing on mainline, and that shared hunk is what kept this
/// change's squash merge colliding. The split is the same one
/// [`elasticbeanstalk_arn_resource`] below performs for this service's own ARNs, so the
/// crate states the six-field form once per direction it reads.
///
/// A value whose form this rejects names no role that could ever be assumed, so it is a
/// defect in the REQUEST — reported as an invalid parameter naming the member and this
/// constraint (see [`service_role_unusable`]).
pub fn is_iam_role_arn(value: &str) -> bool {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        // IAM is partition-global, so an IAM ARN's Region field is empty.
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    // `aws(-[^:]+)?`: the partition is `aws`, or `aws-` plus a non-empty suffix
    // (`aws-cn`, `aws-us-gov`) — not merely a value STARTING with those letters.
    let partition_well_formed = partition == "aws"
        || partition
            .strip_prefix("aws-")
            .is_some_and(|suffix| !suffix.is_empty());
    let account_well_formed = account.is_empty()
        || (account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit()));
    partition_well_formed
        && account_well_formed
        && resource
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty())
}

/// The message a rejected parameter VALUE is reported with: the member the request
/// named and the constraint that member's value failed, which is how AWS words a
/// validation refusal it builds from the request ("Parameter ServiceRole is invalid.
/// Must be a valid IAM Role ARN.").
pub fn invalid_parameter_message(member: &str, requirement: &str) -> String {
    format!("Parameter {member} is invalid. Must be {requirement}.")
}

/// The requirement clause [`invalid_parameter_message`] states for a member documented
/// as the ARN of an IAM role, in the one place both supporting calls read it from.
pub const IAM_ROLE_ARN_REQUIREMENT: &str = "a valid IAM Role ARN";

/// The message the modelled `TooManyTagsException` is answered with: the quota a
/// resource has, which is what the exception states — "The number of tags in the
/// resource would exceed the number of tags that each resource can have."
///
/// Built from [`MAX_TAGS_PER_RESOURCE`], so the sentence cannot state a limit this
/// service does not enforce, and shared by every check that raises the exception: the
/// quota is one fact, so a caller reads one sentence for it however it was exceeded.
pub fn too_many_tags_message() -> String {
    format!(
        "The maximum number of tags per resource is exceeded. The number of allowed tags \
         is {MAX_TAGS_PER_RESOURCE}."
    )
}

/// A stored application.
#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    /// The application's resource lifecycle configuration. NOT optional: the
    /// service applies its own version-lifecycle policy to every application, so
    /// a record always carries one and every read reports it. See
    /// [`resource_lifecycle_config_with_defaults`].
    pub resource_lifecycle_config: ApplicationResourceLifecycleConfig,
    pub tags: Vec<Tag>,
    /// The deletion the clock owes this application, once `DeleteApplication`
    /// has started one. See [`PendingDeletion`].
    pub pending_deletion: Option<PendingDeletion>,
}

/// The deletion of an application the clock owes: when it completes, and the
/// request that asked for it.
///
/// `DeleteApplication` "deletes the specified application along with all
/// associated versions and configurations" asynchronously: the call returns at
/// once and the application is still described while the deletion runs, so a
/// client polling the resource it owns is not told it has already gone. The
/// removal is due one [`STATUS_TRANSITION_SPAN`] after the call that started it
/// and is materialised by the next call that observes it as due — nothing
/// sleeps.
#[derive(Debug, Clone)]
pub struct PendingDeletion {
    pub due_at: Instant,
    /// The same instant on the server clock, so the event the completed deletion
    /// records is dated when the deletion came due.
    pub due_date: ::aws_smithy_types::DateTime,
    /// The request that started the deletion: the completion event belongs to
    /// that request's activity, not to whichever later call observed it.
    pub request_id: String,
}

impl Application {
    /// Start the deletion this application owes: see [`PendingDeletion`].
    pub fn begin_deletion(&mut self, request_id: &str) {
        self.pending_deletion = Some(PendingDeletion {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            request_id: request_id.to_string(),
        });
    }

    /// Whether a deletion of this application has already been started.
    pub fn is_deleting(&self) -> bool {
        self.pending_deletion.is_some()
    }

    /// The modelled `ApplicationDescription` for this record. `versions` and
    /// `configuration_templates` are derived by the caller from
    /// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::version_labels`]
    /// and
    /// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::template_names`],
    /// so neither list is a second copy of the store it is read from.
    pub fn to_description(
        &self,
        versions: Vec<String>,
        configuration_templates: Vec<String>,
    ) -> ApplicationDescription {
        ApplicationDescription {
            application_arn: Some(self.arn.clone()),
            application_name: Some(self.name.clone()),
            description: self.description.clone(),
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            versions: Some(versions),
            configuration_templates: Some(configuration_templates),
            resource_lifecycle_config: Some(self.resource_lifecycle_config.clone()),
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
            // Reported in the wire form the service uses for the status, not in
            // the modelled spelling: see [`version_status_on_the_wire`].
            status: Some(version_status_on_the_wire(&self.status)),
        }
    }
}

/// A stored configuration template — a "saved configuration", the settings a
/// `CreateConfigurationTemplate` call persisted under one `(ApplicationName,
/// TemplateName)` pair.
///
/// A template is NEVER associated with an environment: "Templates aren't
/// associated with any environment. The `EnvironmentName` response element is
/// always null", and `DeploymentStatus` is documented as "null: This
/// configuration is not associated with a running environment" — so neither
/// member is stored here, and the projection below reports both as absent.
#[derive(Debug, Clone)]
pub struct ConfigurationTemplate {
    pub application_name: String,
    pub template_name: String,
    pub description: Option<String>,
    /// The platform version the settings belong to, by solution stack name and
    /// by ARN. Either can be absent: a CUSTOM platform version has no solution
    /// stack name this service can derive, and a template created from a
    /// solution stack the catalogue does not publish has no ARN to report.
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    /// The option settings this configuration holds, in the order they were
    /// supplied, de-duplicated by the key an option is identified by
    /// (`Namespace`, `OptionName`, `ResourceName`).
    pub option_settings: Vec<ConfigurationOptionSetting>,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl ConfigurationTemplate {
    /// The store key for this record: a template name "must be unique per
    /// application", so the pair is the identity.
    pub fn key(&self) -> (String, String) {
        (self.application_name.clone(), self.template_name.clone())
    }

    /// The resources an event about this template belongs to.
    pub fn event_subject(&self) -> EventSubject {
        EventSubject {
            application_name: Some(self.application_name.clone()),
            template_name: Some(self.template_name.clone()),
            platform_arn: self.platform_arn.clone(),
            ..Default::default()
        }
    }
}

/// The key an option setting is identified by — `(Namespace, OptionName,
/// ResourceName)`, the same three members `OptionSpecification` names one by.
/// Two settings sharing it are the SAME option, so the later value replaces the
/// earlier one rather than being appended beside it.
pub fn option_setting_key(setting: &ConfigurationOptionSetting) -> (String, String, String) {
    (
        setting.namespace.clone().unwrap_or_default(),
        setting.option_name.clone().unwrap_or_default(),
        setting.resource_name.clone().unwrap_or_default(),
    )
}

/// The key an option SPECIFICATION identifies an option by — the same
/// `(Namespace, OptionName, ResourceName)` triple [`option_setting_key`] builds
/// from a setting, so a specification that names an option and the setting
/// carrying it compare equal.
///
/// `OptionSpecification` is how a request NAMES an option without giving it a
/// value (`UpdateEnvironment$OptionsToRemove`,
/// `DescribeConfigurationOptions$Options`), which is why it needs a key of its
/// own rather than being compared member by member at each call site.
pub fn option_specification_key(specification: &OptionSpecification) -> (String, String, String) {
    (
        specification.namespace.clone().unwrap_or_default(),
        specification.option_name.clone().unwrap_or_default(),
        specification.resource_name.clone().unwrap_or_default(),
    )
}

/// `settings` without the options `removed` names: "a list of custom
/// user-defined configuration options to remove from the configuration set"
/// (`UpdateEnvironment$OptionsToRemove`). An option the set does not carry is
/// simply not there to remove, so naming it changes nothing.
///
/// Keys are collected into a set ONCE, so the work is linear in the two lists'
/// combined length however long the caller's removal list is.
pub fn remove_option_settings(
    settings: &[ConfigurationOptionSetting],
    removed: &[OptionSpecification],
) -> Vec<ConfigurationOptionSetting> {
    let removed: BTreeSet<(String, String, String)> =
        removed.iter().map(option_specification_key).collect();
    settings
        .iter()
        .filter(|setting| !removed.contains(&option_setting_key(setting)))
        .cloned()
        .collect()
}

/// `base` with every setting in `overrides` applied: a setting whose
/// [`option_setting_key`] `base` already carries replaces that one in place, and
/// one it does not carry is appended. This is what "Values specified in
/// `OptionSettings` override any values obtained from the `SourceConfiguration`"
/// means, and it is also how a request that lists the same option twice settles
/// on one value.
///
/// ONE pass over the two lists, with each element's key computed ONCE and the
/// position of the setting holding it looked up in a map: the work is linear in
/// the combined length, so a caller-chosen list cannot turn into quadratic work
/// inside the store's critical section. The result length is bounded by the
/// caller — see [`MAX_CONFIGURATION_OPTION_SETTINGS`], which the handler refuses
/// a longer request and a longer merged result with.
pub fn merge_option_settings(
    base: &[ConfigurationOptionSetting],
    overrides: &[ConfigurationOptionSetting],
) -> Vec<ConfigurationOptionSetting> {
    let mut merged: Vec<ConfigurationOptionSetting> = Vec::new();
    let mut positions: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for setting in base.iter().chain(overrides.iter()) {
        match positions.get(&option_setting_key(setting)) {
            Some(&at) => merged[at] = setting.clone(),
            None => {
                positions.insert(option_setting_key(setting), merged.len());
                merged.push(setting.clone());
            }
        }
    }
    merged
}

/// Build one of the generated `ConfigurationSettingsDescription` structs from a
/// stored configuration template.
///
/// The model binds `ConfigurationSettingsDescription` as the output of
/// `CreateConfigurationTemplate`, `UpdateConfigurationTemplate` and (as a list
/// member) `DescribeConfigurationSettings`, and the scaffold emits a DISTINCT
/// Rust struct per operation for it with identical members, so the mapping is
/// written once here and the struct is the parameter — the same seam
/// [`environment_description`] provides for `EnvironmentDescription`.
macro_rules! configuration_settings_description {
    ($ty:path, $template:expr) => {{
        let template: &$crate::state::ConfigurationTemplate = $template;
        $ty {
            solution_stack_name: template.solution_stack_name.clone(),
            platform_arn: template.platform_arn.clone(),
            application_name: Some(template.application_name.clone()),
            template_name: Some(template.template_name.clone()),
            description: template.description.clone(),
            // "Templates aren't associated with any environment. The
            // EnvironmentName response element is always null", and a
            // configuration that is not associated with a running environment
            // reports a null `DeploymentStatus`.
            environment_name: None,
            deployment_status: None,
            date_created: Some(template.date_created),
            date_updated: Some(template.date_updated),
            option_settings: Some(template.option_settings.clone()),
        }
    }};
}

pub(crate) use configuration_settings_description;

/// The resources one recorded event is about — exactly the members
/// `DescribeEvents` filters on, so a filter and a recording cannot disagree
/// about which resource an event belongs to.
///
/// `environment_id` is NOT an `EventDescription` member: the wire shape reports
/// only `EnvironmentName`. It is kept here because `DescribeEvents$EnvironmentId`
/// filters on it, and an id identifies the record even after its name has been
/// released by a termination and taken by a new environment.
#[derive(Debug, Clone, Default)]
pub struct EventSubject {
    pub application_name: Option<String>,
    pub version_label: Option<String>,
    pub template_name: Option<String>,
    pub environment_id: Option<String>,
    pub environment_name: Option<String>,
    pub platform_arn: Option<String>,
}

/// One recorded event: what happened, when, how severe it was, which request
/// produced it and which resources it is about.
#[derive(Debug, Clone)]
pub struct Event {
    /// Append order within this emulator, so "most recent first" is a total
    /// order even for events recorded inside one clock tick, and so a
    /// `NextToken` names one event unambiguously.
    pub sequence: u64,
    pub event_date: ::aws_smithy_types::DateTime,
    pub message: String,
    pub severity: String,
    pub request_id: String,
    pub subject: EventSubject,
}

impl Event {
    /// The modelled `EventDescription` for this record.
    pub fn to_description(&self) -> EventDescription {
        EventDescription {
            event_date: Some(self.event_date),
            message: Some(self.message.clone()),
            application_name: self.subject.application_name.clone(),
            version_label: self.subject.version_label.clone(),
            template_name: self.subject.template_name.clone(),
            environment_name: self.subject.environment_name.clone(),
            platform_arn: self.subject.platform_arn.clone(),
            request_id: Some(self.request_id.clone()),
            severity: Some(self.severity.clone()),
        }
    }
}

/// One managed action this service has CARRIED OUT on an environment — "the
/// record of a completed or failed managed action" (the model's
/// `ManagedActionHistoryItem` doc), which is what
/// `DescribeEnvironmentManagedActionHistory` reports.
///
/// A managed action is work the SERVICE performs on an environment on its own
/// maintenance schedule (a managed platform update, an instance refresh), not
/// work a caller asked for: a caller-requested change is reported in the event
/// log ([`Event`]) instead. An action that has NOT finished is not held here —
/// an upcoming or in-progress one is the model's `ManagedAction`, which
/// `DescribeEnvironmentManagedActions` reports — so every record in this log is
/// terminal and its [`Self::status`] is one of the modelled
/// `crate::ops::types::ACTION_HISTORY_STATUS_VALUES`.
///
/// NOTHING writes a record here: a managed action exists because the service's
/// maintenance-window scheduler scheduled one, and this emulator runs no such
/// scheduler — which is also why `ApplyEnvironmentManagedAction` refuses every
/// `ActionId` it is given, no action ever being `Scheduled`. So no action has
/// ever executed on an environment this store holds, and
/// `DescribeEnvironmentManagedActionHistory` pages over THIS log rather than
/// reporting an action nothing performed.
#[derive(Debug, Clone)]
pub struct ManagedActionRecord {
    /// Append order within this emulator, so "most recent first" is a total
    /// order even for two actions that finished inside one clock tick, and so a
    /// `NextToken` names one record unambiguously. Drawn from the store's
    /// shared counter, as an [`Event`]'s is.
    pub sequence: u64,
    /// The environment the action ran on, by `EnvironmentId`: the id identifies
    /// the record even after its name has been released by a termination and
    /// taken by a new environment.
    pub environment_id: String,
    /// "A unique identifier for the managed action."
    pub action_id: String,
    /// "The type of the managed action" — one of the modelled
    /// `crate::ops::types::ACTION_TYPE_VALUES`.
    pub action_type: String,
    /// "A description of the managed action."
    pub action_description: Option<String>,
    /// "The status of the action" — one of the modelled
    /// `crate::ops::types::ACTION_HISTORY_STATUS_VALUES`.
    pub status: String,
    /// "If the action failed, the type of failure" — one of the modelled
    /// `crate::ops::types::FAILURE_TYPE_VALUES`, and absent for an action that
    /// completed.
    pub failure_type: Option<String>,
    /// "If the action failed, a description of the failure."
    pub failure_description: Option<String>,
    /// "The date and time that the action started executing."
    pub executed_time: ::aws_smithy_types::DateTime,
    /// "The date and time that the action finished executing."
    pub finished_time: Option<::aws_smithy_types::DateTime>,
}

impl ManagedActionRecord {
    /// The instant this record is ORDERED by, as an exact
    /// (seconds, nanoseconds) key: when the action finished, which is why it is
    /// in the history at all, falling back to when it started for a record that
    /// carries no finish instant.
    pub fn history_instant(&self) -> (i64, u32) {
        instant_key(self.finished_time.as_ref().unwrap_or(&self.executed_time))
    }

    /// The modelled `ManagedActionHistoryItem` for this record.
    pub fn to_history_item(&self) -> ManagedActionHistoryItem {
        ManagedActionHistoryItem {
            action_id: Some(self.action_id.clone()),
            action_type: Some(self.action_type.clone()),
            action_description: self.action_description.clone(),
            failure_type: self.failure_type.clone(),
            status: Some(self.status.clone()),
            failure_description: self.failure_description.clone(),
            executed_time: Some(self.executed_time),
            finished_time: self.finished_time,
        }
    }
}

/// A stored custom platform version.
#[derive(Debug, Clone)]
pub struct PlatformVersion {
    pub arn: String,
    pub platform_name: String,
    pub platform_version: String,
    /// "PlatformOwner: The AWS account ID of the person who created the platform
    /// version" — this account, for a platform version created here.
    pub platform_owner: String,
    pub status: String,
    /// Where the platform definition archive was said to be. Nothing reads the
    /// archive, so none of the values a built platform would report from it (its
    /// category, maintainer, operating system, languages, frameworks) is
    /// invented.
    pub definition_bundle: Option<S3Location>,
    pub option_settings: Vec<ConfigurationOptionSetting>,
    /// The builder environment the caller named, carried so the record says what
    /// was asked for. No builder environment is launched here.
    pub builder_environment_name: Option<String>,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
    /// The move to `Ready` the clock owes this version. See [`PendingPlatformStatus`].
    pub pending_status: Option<PendingPlatformStatus>,
}

/// The status change the clock owes a platform version: `Creating` -> `Ready`,
/// due one [`STATUS_TRANSITION_SPAN`] after the call that started it.
#[derive(Debug, Clone)]
pub struct PendingPlatformStatus {
    pub due_at: Instant,
    pub status: String,
}

impl PendingPlatformStatus {
    /// The move `CreatePlatformVersion` owes: `Creating` -> `Ready`.
    pub fn build_completion() -> Self {
        Self {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            status: PLATFORM_STATUS_READY.to_string(),
        }
    }
}

impl PlatformVersion {
    /// Apply the pending status change if the clock has already made it due — the
    /// same read-time materialisation [`Environment::materialize_due`] performs.
    fn materialize_due(&mut self, at: Instant) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if pending.due_at > at {
            return;
        }
        let pending = self.pending_status.take().expect("checked above");
        self.status = pending.status;
        self.date_updated = now();
    }

    /// Whether this version can be launched into an environment.
    pub fn is_ready(&self) -> bool {
        self.status == PLATFORM_STATUS_READY
    }

    /// The modelled `PlatformSummary` for this record.
    pub fn to_summary(&self) -> PlatformSummary {
        PlatformSummary {
            platform_arn: Some(self.arn.clone()),
            platform_owner: Some(self.platform_owner.clone()),
            platform_status: Some(self.status.clone()),
            // Everything below is read off a built platform's definition archive,
            // which this emulator does not read; a platform version it cannot
            // derive reports no value rather than a guessed one. A custom version
            // belongs to no published branch either, so it has no branch name and
            // no branch life-cycle state.
            platform_category: None,
            operating_system_name: None,
            operating_system_version: None,
            supported_tier_list: None,
            supported_addon_list: None,
            platform_lifecycle_state: None,
            platform_version: Some(self.platform_version.clone()),
            platform_branch_name: None,
            platform_branch_lifecycle_state: None,
        }
    }

    /// This record as `ListPlatformVersions` reports and filters it. A custom
    /// version states no programming language: that is read off the platform
    /// definition archive, which this service does not read.
    pub fn to_listing(&self) -> ListedPlatformVersion {
        ListedPlatformVersion {
            summary: self.to_summary(),
            programming_language_names: Vec::new(),
        }
    }

    /// The modelled `PlatformDescription` for this record.
    pub fn to_description(&self) -> PlatformDescription {
        PlatformDescription {
            platform_arn: Some(self.arn.clone()),
            platform_owner: Some(self.platform_owner.clone()),
            platform_name: Some(self.platform_name.clone()),
            platform_version: Some(self.platform_version.clone()),
            platform_status: Some(self.status.clone()),
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            // As in [`Self::to_summary`]: not derivable without reading the
            // platform definition archive.
            solution_stack_name: None,
            platform_category: None,
            description: None,
            maintainer: None,
            operating_system_name: None,
            operating_system_version: None,
            programming_languages: None,
            frameworks: None,
            custom_ami_list: None,
            supported_tier_list: None,
            supported_addon_list: None,
            platform_lifecycle_state: None,
            platform_branch_name: None,
            platform_branch_lifecycle_state: None,
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

/// A status change the clock owes an environment: what it moves to, when that
/// move is due, and the event the move records. See [`STATUS_TRANSITION_SPAN`].
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub due_at: Instant,
    /// The same instant on the server clock, so the record and the event the
    /// change records are dated when the change came DUE rather than when a
    /// later call happened to observe it as due.
    pub due_date: ::aws_smithy_types::DateTime,
    pub status: String,
    pub health: String,
    pub health_status: Option<String>,
    /// What `AbortableOperationInProgress` reports once the change is
    /// materialised. The flag is part of the state the clock owes: an operation
    /// the clock has finished is no longer one "you can cancel", so it is
    /// carried here beside the status rather than left at whatever the call that
    /// STARTED the operation set.
    pub abortable_operation_in_progress: bool,
    /// The request that started the change. The event the completion records
    /// belongs to that request's activity, not to whichever later call happened
    /// to observe the change as due.
    pub request_id: String,
    /// The message of the event recorded when the move is materialised.
    pub completion_message: String,
}

impl PendingStatus {
    /// The move an in-progress operation on a surviving environment owes once it
    /// finishes: to `Ready` ("Environment is available to have an action
    /// performed on it"), reporting healthy ("Green: Indicates the environment
    /// is healthy and fully functional"). Both the launch CreateEnvironment
    /// starts and the cancellation AbortEnvironmentUpdate starts end here, so
    /// the two cannot drift apart; they differ only in the request the
    /// completion belongs to and the event it records.
    pub fn completion_to_ready(request_id: &str, completion_message: impl Into<String>) -> Self {
        Self {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: STATUS_READY.to_string(),
            health: HEALTH_GREEN.to_string(),
            health_status: Some(HEALTH_STATUS_OK.to_string()),
            // The operation the completion ends is over, so there is nothing
            // left to cancel.
            abortable_operation_in_progress: false,
            request_id: request_id.to_string(),
            completion_message: completion_message.into(),
        }
    }

    /// The move CreateEnvironment owes: `Launching` -> `Ready`, recording that
    /// the launch the call started has finished.
    pub fn launch_completion(request_id: &str, environment_name: &str) -> Self {
        Self::completion_to_ready(
            request_id,
            format!("Successfully launched environment: {environment_name}"),
        )
    }
}

/// One [`STATUS_TRANSITION_SPAN`] from now, on the server clock.
///
/// Microsecond-aligned like [`now`] itself — the span is a whole number of
/// milliseconds, so adding it keeps the stored instant one a `date-time`
/// rendering can express exactly.
fn due_date() -> ::aws_smithy_types::DateTime {
    let at = now();
    let nanos =
        u64::from(at.subsec_nanos()) + u64::from(STATUS_TRANSITION_SPAN.subsec_micros()) * 1_000;
    let seconds = at.secs()
        + i64::try_from(STATUS_TRANSITION_SPAN.as_secs() + nanos / 1_000_000_000)
            .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        seconds,
        u32::try_from(nanos % 1_000_000_000).unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// The Amazon Web Services resources an environment runs on
//
// `CreateEnvironment` "launches an environment for the specified application",
// which is what provisions them; `DescribeEnvironmentResources` reports them
// ("Describes the Amazon Web Services resources in use by this environment");
// `RebuildEnvironment` "deletes and recreates all of the Amazon Web Services
// resources … for a specified environment", which REPLACES every one of them;
// and an environment the clock has moved to `Terminated` ("Environment is not
// running") has none left.
//
// The inventory is STORED on the environment rather than derived per call: the
// names and ids the real service reports are server-generated, so two reads of
// one environment have to report the same ones, and a rebuild is what changes
// them. Which KINDS of resource an environment has is derived from the
// environment's tier and its configuration options (below), so the inventory
// follows what the caller asked to launch.
// ---------------------------------------------------------------------------

/// The configuration-option namespace Elastic Beanstalk's published
/// configuration-option reference puts the environment-wide options in, and the
/// option in it that says whether the environment is load balanced
/// ("`EnvironmentType`: Set to `SingleInstance` to launch one EC2 instance with
/// no load balancer"; the documented default is `LoadBalanced`).
const NAMESPACE_ENVIRONMENT: &str = "aws:elasticbeanstalk:environment";
/// See [`NAMESPACE_ENVIRONMENT`].
const OPTION_ENVIRONMENT_TYPE: &str = "EnvironmentType";

/// The `aws:elasticbeanstalk:environment` / `EnvironmentType` value that
/// launches "one EC2 instance with no load balancer".
pub const ENVIRONMENT_TYPE_SINGLE_INSTANCE: &str = "SingleInstance";
/// The other value of that option, and its documented DEFAULT: an environment
/// behind a load balancer, whose Auto Scaling group scales between its
/// configured bounds.
pub const ENVIRONMENT_TYPE_LOAD_BALANCED: &str = "LoadBalanced";

/// The Auto Scaling namespace of that same reference, and the option in it that
/// gives "the minimum number of instances you want in your Auto Scaling group"
/// — which is how many Amazon EC2 instances a freshly launched environment
/// runs, nothing having scaled it yet.
const NAMESPACE_AUTOSCALING_ASG: &str = "aws:autoscaling:asg";
/// See [`NAMESPACE_AUTOSCALING_ASG`].
const OPTION_MIN_SIZE: &str = "MinSize";

/// `aws:autoscaling:asg` / `MinSize`'s documented default, and the number of
/// instances a `SingleInstance` environment runs ("one EC2 instance").
pub const DEFAULT_MIN_SIZE: usize = 1;

/// How many instances one environment's inventory can hold here.
///
/// The option is a free-form string in the API, so a request can ask for an
/// Auto Scaling minimum the real service would refuse against an account's
/// Amazon EC2 quota. This bound is CHOSEN — it is what keeps one environment's
/// stored inventory bounded — and a larger `MinSize` is held down to it rather
/// than materialised.
pub const MAX_ENVIRONMENT_INSTANCES: usize = 20;

/// The logical id of the Auto Scaling group in the Amazon CloudFormation stack
/// Elastic Beanstalk provisions an environment's resources through, as that
/// stack names it. Every environment has one: a `SingleInstance` environment's
/// group is simply bounded at one instance.
pub const STACK_RESOURCE_AUTO_SCALING_GROUP: &str = "AWSEBAutoScalingGroup";
/// The logical id of the environment's load balancer in that stack. Present
/// only for a load-balanced web-server environment.
pub const STACK_RESOURCE_LOAD_BALANCER: &str = "AWSEBLoadBalancer";
/// The logical ids of the Auto Scaling triggers in that stack — the
/// scale-up/scale-down alarms a group that can scale is driven by, which is why
/// a `SingleInstance` environment has none.
pub const STACK_RESOURCE_TRIGGERS: &[&str] =
    &["AWSEBCloudwatchAlarmHigh", "AWSEBCloudwatchAlarmLow"];
/// The logical id of the worker environment's Amazon SQS queue in that stack —
/// the queue the `Worker` tier's daemon reads, which a `WebServer` environment
/// does not have.
pub const STACK_RESOURCE_WORKER_QUEUE: &str = "AWSEBWorkerQueue";

/// How many characters the random suffix a stack resource's physical name ends
/// in carries. Upper-case, as the stack's own physical names are.
const STACK_RESOURCE_SUFFIX_LENGTH: usize = 13;

/// How many characters an Amazon EC2 resource id's body carries: `i-` or `lt-`
/// followed by 17 lowercase hexadecimal characters is the form Amazon EC2
/// issues an instance id and a launch template id in.
pub const EC2_RESOURCE_ID_BODY_LENGTH: usize = 17;

/// The id prefix of an Amazon EC2 instance.
pub const EC2_INSTANCE_ID_PREFIX: &str = "i";
/// The id prefix of an Amazon EC2 launch template.
pub const EC2_LAUNCH_TEMPLATE_ID_PREFIX: &str = "lt";

/// The Amazon CloudFormation stack an environment's Amazon Web Services
/// resources are created in: `awseb-<environment id>-stack`, the prefix every
/// one of those resources' own names then carries.
pub fn environment_stack_name(environment_id: &str) -> String {
    format!("awseb-{environment_id}-stack")
}

/// The physical name of the `logical_id` resource of `stack`:
/// `<stack>-<logical id>-<suffix>`, the form a stack's resource is named in.
///
/// The suffix is random, the way the real one is: it identifies THAT resource,
/// so the resource a rebuild creates in place of an old one carries a different
/// name — which is what makes "deletes and recreates all of the Amazon Web
/// Services resources" observable.
fn stack_resource_name(stack: &str, logical_id: &str) -> String {
    format!(
        "{stack}-{logical_id}-{}",
        random_id_body(STACK_RESOURCE_SUFFIX_LENGTH).to_ascii_uppercase()
    )
}

/// A fresh Amazon EC2 resource id with `prefix`: `<prefix>-<17 lowercase
/// hexadecimal characters>`. Random, like the stack resource names above and
/// for the same reason.
fn ec2_resource_id(prefix: &str) -> String {
    format!("{prefix}-{}", random_id_body(EC2_RESOURCE_ID_BODY_LENGTH))
}

/// The URL of the Amazon SQS queue `queue_name` in `region`:
/// `https://sqs.<region>.amazonaws.com/<account>/<queue name>`, the form Amazon
/// SQS reports a queue's URL in.
///
/// This is the queue Elastic Beanstalk's own stack holds for the environment, so
/// the URL is the one the control plane reports for it; nothing here registers
/// that queue with the emulated Amazon SQS, which would be a cross-service
/// provisioning step of its own.
pub fn queue_url(region: &str, queue_name: &str) -> String {
    format!(
        "https://sqs.{region}.amazonaws.com/{}/{queue_name}",
        ::engine::emulator_account_id()
    )
}

/// The value of the `namespace` / `option_name` configuration option in
/// `settings`, if the environment carries one. A namespace and an option name
/// together identify one option, so the first match is the only match.
fn option_value<'a>(
    settings: &'a [ConfigurationOptionSetting],
    namespace: &str,
    option_name: &str,
) -> Option<&'a str> {
    settings
        .iter()
        .find(|setting| {
            setting.namespace.as_deref() == Some(namespace)
                && setting.option_name.as_deref() == Some(option_name)
        })
        .and_then(|setting| setting.value.as_deref())
}

/// Whether the environment these options configure sits behind a load balancer:
/// every value of `EnvironmentType` other than `SingleInstance` does, and so
/// does an environment that sets the option at all (`LoadBalanced` is the
/// option's documented default).
fn is_load_balanced(settings: &[ConfigurationOptionSetting]) -> bool {
    option_value(settings, NAMESPACE_ENVIRONMENT, OPTION_ENVIRONMENT_TYPE)
        .is_none_or(|environment_type| environment_type != ENVIRONMENT_TYPE_SINGLE_INSTANCE)
}

/// Whether `tier` is the worker tier — the tier whose environment "pulls tasks
/// from an Amazon SQS queue", and so the only one with a queue. Decided on
/// `EnvironmentTier$Name`, which is the member that names the tier
/// ([`TIER_NAME_WORKER`]); `Type` carries the transport.
fn is_worker_tier(tier: Option<&EnvironmentTier>) -> bool {
    tier.and_then(|tier| tier.name.as_deref()) == Some(TIER_NAME_WORKER)
}

/// How many Amazon EC2 instances an environment with these options runs: its
/// Auto Scaling minimum, held inside `1..=`[`MAX_ENVIRONMENT_INSTANCES`]. A
/// `SingleInstance` environment runs exactly one whatever the option says,
/// since that environment type IS one instance.
///
/// A value that is not a count at all (a word, a negative number) is treated as
/// the option's own default, the same as not setting it: validating one
/// configuration option's syntax belongs to the operations that TAKE option
/// settings (`ValidateConfigurationSettings`, `UpdateEnvironment`), not to the
/// report that reads the environment they stored.
fn instance_count(load_balanced: bool, settings: &[ConfigurationOptionSetting]) -> usize {
    if !load_balanced {
        return DEFAULT_MIN_SIZE;
    }
    option_value(settings, NAMESPACE_AUTOSCALING_ASG, OPTION_MIN_SIZE)
        .and_then(|min_size| min_size.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MIN_SIZE)
        .clamp(DEFAULT_MIN_SIZE, MAX_ENVIRONMENT_INSTANCES)
}

/// One Amazon SQS queue an environment uses: "the name of the queue" and "the
/// URL of the queue", the two members `Queue` carries.
#[derive(Debug, Clone)]
pub struct EnvironmentQueue {
    pub name: String,
    pub url: String,
}

/// The Amazon Web Services resources Elastic Beanstalk has provisioned for one
/// environment — what `DescribeEnvironmentResources` reports.
///
/// Every live environment has an Auto Scaling group, the Amazon EC2 launch
/// template that group launches from, and its instances. The rest depends on
/// what was launched: a load-balanced environment also has the Auto Scaling
/// triggers its group scales on, a load-balanced WEB SERVER environment a load
/// balancer, and a WORKER environment the queue its daemon reads.
///
/// There is no launch CONFIGURATION: Amazon EC2 Auto Scaling no longer creates
/// them, and the platform branches this service publishes
/// ([`SOLUTION_STACKS`]) are the Amazon Linux 2023 ones Elastic Beanstalk
/// launches from a launch TEMPLATE.
#[derive(Debug, Clone)]
pub struct EnvironmentResources {
    pub auto_scaling_group: String,
    pub launch_template_id: String,
    pub instances: Vec<String>,
    pub triggers: Vec<String>,
    pub load_balancer: Option<String>,
    pub queues: Vec<EnvironmentQueue>,
}

impl EnvironmentResources {
    /// Provision a fresh set of resources for the environment `environment_id`
    /// in `region`, for the tier and the configuration options it was launched
    /// with. Called when the environment is created and again whenever a
    /// rebuild recreates them, so the two cannot disagree about what an
    /// environment is made of.
    pub fn provision(
        environment_id: &str,
        region: &str,
        tier: Option<&EnvironmentTier>,
        option_settings: &[ConfigurationOptionSetting],
    ) -> Self {
        let stack = environment_stack_name(environment_id);
        let load_balanced = is_load_balanced(option_settings);
        let worker = is_worker_tier(tier);
        Self {
            auto_scaling_group: stack_resource_name(&stack, STACK_RESOURCE_AUTO_SCALING_GROUP),
            launch_template_id: ec2_resource_id(EC2_LAUNCH_TEMPLATE_ID_PREFIX),
            instances: (0..instance_count(load_balanced, option_settings))
                .map(|_| ec2_resource_id(EC2_INSTANCE_ID_PREFIX))
                .collect(),
            // A group bounded at one instance never scales, so it is driven by
            // no trigger.
            triggers: if load_balanced {
                STACK_RESOURCE_TRIGGERS
                    .iter()
                    .map(|logical_id| stack_resource_name(&stack, logical_id))
                    .collect()
            } else {
                Vec::new()
            },
            // A worker environment takes its work from its queue rather than
            // from a load balancer, so it has none either.
            load_balancer: (load_balanced && !worker)
                .then(|| stack_resource_name(&stack, STACK_RESOURCE_LOAD_BALANCER)),
            queues: if worker {
                let name = stack_resource_name(&stack, STACK_RESOURCE_WORKER_QUEUE);
                vec![EnvironmentQueue {
                    url: queue_url(region, &name),
                    name,
                }]
            } else {
                Vec::new()
            },
        }
    }

    /// Bring this inventory in line with the configuration options the
    /// environment now carries, keeping every resource the new configuration
    /// still calls for under the name it was provisioned with.
    ///
    /// `UpdateEnvironment` "updates the configuration settings in the running
    /// environment"; the operation that "deletes and recreates all of the Amazon
    /// Web Services resources … for a specified environment" is
    /// `RebuildEnvironment`. So an update is NOT a reprovision: the Auto Scaling
    /// group, the launch template and the worker queue keep their names, and
    /// what the options DECIDE follows the options — how many instances the
    /// group holds (`aws:autoscaling:asg` / `MinSize`), whether it has the
    /// triggers a group that can scale is driven by, and whether the environment
    /// sits behind a load balancer (`aws:elasticbeanstalk:environment` /
    /// `EnvironmentType`). An instance the group no longer holds is gone and a
    /// new one gets its own id, the way a scaling group's membership changes.
    ///
    /// The tier is passed because the load balancer depends on it, and an
    /// environment's tier cannot be changed by an update, so the queue a worker
    /// environment reads is untouched.
    pub fn reconfigure(
        &mut self,
        environment_id: &str,
        tier: Option<&EnvironmentTier>,
        option_settings: &[ConfigurationOptionSetting],
    ) {
        let stack = environment_stack_name(environment_id);
        let load_balanced = is_load_balanced(option_settings);
        let wanted = instance_count(load_balanced, option_settings);
        self.instances.truncate(wanted);
        while self.instances.len() < wanted {
            self.instances.push(ec2_resource_id(EC2_INSTANCE_ID_PREFIX));
        }
        if !load_balanced {
            self.triggers.clear();
            self.load_balancer = None;
            return;
        }
        if self.triggers.is_empty() {
            self.triggers = STACK_RESOURCE_TRIGGERS
                .iter()
                .map(|logical_id| stack_resource_name(&stack, logical_id))
                .collect();
        }
        if self.load_balancer.is_none() && !is_worker_tier(tier) {
            self.load_balancer = Some(stack_resource_name(
                &stack,
                STACK_RESOURCE_LOAD_BALANCER,
            ));
        }
    }
}

/// A stored environment.
#[derive(Debug, Clone)]
pub struct Environment {
    pub environment_id: String,
    pub environment_name: String,
    pub arn: String,
    /// The region the request that created this environment was addressed to —
    /// the one its ARN, its CNAME and its resources' own names are built in.
    pub region: String,
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
    /// The Amazon Web Services resources Elastic Beanstalk has provisioned for
    /// this environment, which `DescribeEnvironmentResources` reports. `None`
    /// once the environment has reached `Terminated`: "Environment is not
    /// running", so there is nothing left for it to be made of.
    pub resources: Option<EnvironmentResources>,
    /// Creation order within this emulator. An `EnvironmentId` is an OPAQUE
    /// random id, so the store's key order is not the order the environments
    /// were created in; this is, which is what `DescribeEnvironments` reports
    /// and pages in so two calls cannot disagree about the order.
    pub sequence: u64,
}

impl Environment {
    /// Whether this environment has reached `Terminated` — the state
    /// DescribeEnvironments hides unless the caller asks for deleted ones.
    pub fn is_terminated(&self) -> bool {
        self.status == STATUS_TERMINATED
    }

    /// Provision a FRESH set of Amazon Web Services resources for this
    /// environment, replacing whatever it held.
    ///
    /// The launch `CreateEnvironment` starts provisions them, and the rebuild
    /// `RebuildEnvironment` starts — which "deletes and recreates all of the
    /// Amazon Web Services resources … for a specified environment" — provisions
    /// them again, so both reach the inventory the same way and a resource a
    /// rebuild replaced is reported under its new name.
    pub fn provision_resources(&mut self) {
        self.resources = Some(EnvironmentResources::provision(
            &self.environment_id,
            &self.region,
            self.tier.as_ref(),
            &self.option_settings,
        ));
    }

    /// Whether this environment is in the one status the model documents as
    /// accepting an action: "Ready: Environment is available to have an action
    /// performed on it, such as update or terminate."
    pub fn is_ready(&self) -> bool {
        self.status == STATUS_READY
    }

    /// The resources an event about this environment belongs to.
    pub fn event_subject(&self) -> EventSubject {
        EventSubject {
            application_name: Some(self.application_name.clone()),
            version_label: self.version_label.clone(),
            template_name: self.template_name.clone(),
            environment_id: Some(self.environment_id.clone()),
            environment_name: Some(self.environment_name.clone()),
            platform_arn: self.platform_arn.clone(),
        }
    }

    /// Start the move to `Terminating`, with the move to `Terminated` due one
    /// [`STATUS_TRANSITION_SPAN`] from now. `request_id` is the request asking
    /// for the termination, which the completion event is then attributed to.
    pub fn begin_termination(&mut self, request_id: &str) {
        self.status = STATUS_TERMINATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = false;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: STATUS_TERMINATED.to_string(),
            health: HEALTH_GREY.to_string(),
            // A terminated environment runs nothing, so it reports no health
            // status at all rather than one of the running values.
            health_status: None,
            abortable_operation_in_progress: false,
            request_id: request_id.to_string(),
            completion_message: "terminateEnvironment completed successfully.".to_string(),
        });
    }

    /// Start a rebuild: RebuildEnvironment "deletes and recreates all of the
    /// Amazon Web Services resources … for a specified environment and forces a
    /// restart", so the environment goes back through the same deployment the
    /// model describes for a new one — "Launching: Environment is in the process
    /// of initial deployment" — and the move back to `Ready` is what the clock
    /// then owes, exactly as after CreateEnvironment.
    pub fn begin_rebuild(&mut self, request_id: &str) {
        self.status = STATUS_LAUNCHING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.date_updated = now();
        // "Deletes and recreates all of the Amazon Web Services resources (for
        // example: the Auto Scaling group, load balancer, etc.) for a specified
        // environment": every resource the environment reported is replaced by
        // the one created in its place.
        self.provision_resources();
        self.pending_status = Some(PendingStatus::completion_to_ready(
            request_id,
            "rebuildEnvironment completed successfully.",
        ));
    }

    /// Start an application-server restart: the environment keeps its status —
    /// RestartAppServer restarts the container server, it does not redeploy —
    /// while its health checks are suspended, which the model's `Health` doc
    /// gives as the `Grey` case ("health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request"). The health the
    /// environment reported before the restart is what the clock owes back.
    pub fn begin_app_server_restart(&mut self, request_id: &str) {
        let restored = PendingStatus {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: self.status.clone(),
            health: self.health.clone(),
            health_status: self.health_status.clone(),
            // A restart neither starts nor ends a cancellable operation, so the
            // flag the environment reported is what it reports after it.
            abortable_operation_in_progress: self.abortable_operation_in_progress,
            request_id: request_id.to_string(),
            completion_message: "restartAppServer completed successfully.".to_string(),
        };
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.date_updated = now();
        self.pending_status = Some(restored);
    }

    /// Start the cancellation of the in-progress update this environment
    /// reports: the environment moves to `Aborting` ("Environment is in the
    /// process of aborting a deployment"), stops reporting an abortable
    /// operation — the one in flight is already being cancelled — and the
    /// return to `Ready` is what the clock then owes, one
    /// [`STATUS_TRANSITION_SPAN`] later. `request_id` is the request asking for
    /// the cancellation, which the completion event is then attributed to.
    ///
    /// Health goes `Grey`/`Pending` for the duration, as the model documents
    /// for an environment whose "health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request".
    pub fn begin_abort(&mut self, request_id: &str) {
        self.status = STATUS_ABORTING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = false;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus::completion_to_ready(
            request_id,
            "abortEnvironmentUpdate completed successfully.",
        ));
    }

    /// Start the update `UpdateEnvironment` asks for: the environment moves to
    /// `Updating` ("Environment is in the process of updating its configuration
    /// settings or application version"), reports an abortable operation in
    /// progress — `AbortableOperationInProgress` is "true: There is an update in
    /// progress", which is what `AbortEnvironmentUpdate` then cancels — and the
    /// return to `Ready` is what the clock owes one [`STATUS_TRANSITION_SPAN`]
    /// later. `request_id` is the request asking for the update, which the
    /// completion event is then attributed to.
    ///
    /// Health goes `Grey`/`Pending` for the duration, as the model documents for
    /// an environment whose "health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request".
    pub fn begin_update(&mut self, request_id: &str) {
        self.status = STATUS_UPDATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = true;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus::completion_to_ready(
            request_id,
            "updateEnvironment completed successfully.",
        ));
    }

    /// Bring the Amazon Web Services resources this environment runs on in line
    /// with the configuration it now carries, KEEPING the identity of every
    /// resource the new configuration still calls for. See
    /// [`EnvironmentResources::reconfigure`]. An environment with no inventory —
    /// one the clock has terminated — has nothing to reconfigure.
    pub fn reconfigure_resources(&mut self) {
        // The inventory is taken out of the record so the reconfiguration can
        // read the rest of it; it goes straight back.
        if let Some(mut resources) = self.resources.take() {
            resources.reconfigure(
                &self.environment_id,
                self.tier.as_ref(),
                &self.option_settings,
            );
            self.resources = Some(resources);
        }
    }

    /// Apply the pending status change if the clock has already made it due, and
    /// answer the completion that change records (`None` when nothing was due).
    ///
    /// This is the one write a reporting operation performs: the change was owed
    /// at `due_at` whether or not anybody called, so materialising it on read
    /// cannot show a caller a state the clock had not already reached. The event
    /// is owed by the same instant, for the same reason, and is dated by it —
    /// never by the moment some later call noticed.
    fn materialize_due(&mut self, at: Instant) -> Option<CompletedTransition> {
        let pending = self.pending_status.as_ref()?;
        if pending.due_at > at {
            return None;
        }
        let pending = self.pending_status.take().expect("checked above");
        self.status = pending.status;
        self.health = pending.health;
        self.health_status = pending.health_status;
        self.abortable_operation_in_progress = pending.abortable_operation_in_progress;
        self.date_updated = pending.due_date;
        if self.status == STATUS_TERMINATED {
            self.date_terminated = Some(pending.due_date);
            // The termination the clock has now completed deleted the
            // environment's Amazon Web Services resources with it, so there are
            // none left to report.
            self.resources = None;
        }
        Some(CompletedTransition {
            request_id: pending.request_id,
            message: pending.completion_message,
            at: pending.due_date,
            subject: self.event_subject(),
        })
    }
}

/// A status change the clock has made due and this store has applied, and the
/// event that change records: the request it belongs to, its message, the
/// instant it came due and the resources it is about.
#[derive(Debug, Clone)]
struct CompletedTransition {
    request_id: String,
    message: String,
    at: ::aws_smithy_types::DateTime,
    subject: EventSubject,
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
            // `EnvironmentDescription$Resources` carries the environment's load
            // balancer by its DNS `Domain` and its `Listeners`, which this
            // emulator serves no traffic on and so reports no `EndpointURL` for
            // either; the resources the environment is MADE of are what
            // `DescribeEnvironmentResources` reports, from the inventory on this
            // same record. Nothing links environments here, so there are no
            // environment links to report.
            resources: None,
            tier: env.tier.clone(),
            environment_links: None,
            environment_arn: Some(env.arn.clone()),
            operations_role: env.operations_role.clone(),
        }
    }};
}

pub(crate) use environment_description;

/// `PlatformLifecycleState` for the newest version of a branch.
///
/// The model spells this value twice and differently — `PlatformSummary`
/// documents "Possible values: `recommended` | empty", `PlatformDescription`
/// "Possible values: `Recommended` | `null`" — and the recorded
/// platform-version responses of the real service carry the CAPITALIZED form on
/// both shapes, so that is the value a platform version reports. A version that
/// is not its branch's recommended one reports no value at all.
pub const PLATFORM_LIFECYCLE_RECOMMENDED: &str = "Recommended";

/// Order two version strings, and more generally two platform attribute values.
///
/// A dotted-numeric value — a `PlatformVersion` such as `4.0.11`, an
/// `OperatingSystemVersion` — is ordered component by component NUMERICALLY, so
/// `4.0.11` is above `4.0.9` as a version ordering requires, and a shorter
/// prefix (`4.0`) below a longer value that extends it (`4.0.1`). A value that
/// is not dotted-numeric orders by bytes among its own kind and AFTER every
/// dotted-numeric one.
///
/// That last rule is what makes this a TOTAL order, which `sort_by` requires:
/// comparing numerically within one kind and by bytes across kinds would not be
/// transitive (`10` > `9` numerically, `9` > `1a` by bytes, yet `10` < `1a` by
/// bytes), and a caller can put such a mix in the store, since
/// `CreatePlatformVersion` takes `PlatformVersion` as a string. The ordering is
/// equivalent to sorting on the key `(is not dotted-numeric, components, bytes)`.
///
/// The newest version of a platform branch is decided by this ordering, and so
/// are `ListPlatformVersions`' `<`/`<=`/`>`/`>=` filter operators, so the two
/// cannot disagree.
pub fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    match (numeric_components(left), numeric_components(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.cmp(right),
    }
}

/// `value`'s `.`-separated components when every one of them is a decimal
/// number, `None` otherwise.
fn numeric_components(value: &str) -> Option<Vec<u64>> {
    if value.is_empty() {
        return None;
    }
    value
        .split('.')
        .map(|component| component.parse::<u64>().ok())
        .collect()
}

/// One platform version as `ListPlatformVersions` reports AND filters it: the
/// modelled `PlatformSummary` the response carries, plus the names of the
/// programming languages the version runs.
///
/// The languages ride alongside because `PlatformFilter$Type` names
/// `ProgrammingLanguageName` as a platform-version attribute a term may select
/// on, while `PlatformSummary` models no language member — so a summary alone
/// cannot answer such a term, and the catalog's languages would be unreachable
/// to the filter.
#[derive(Debug, Clone)]
pub struct ListedPlatformVersion {
    pub summary: PlatformSummary,
    pub programming_language_names: Vec<String>,
}

/// The branch named `branch_name` in `branches`, if the catalog holds it: the
/// ONE branch-by-name lookup, so a caller holding the catalog map and a caller
/// holding the whole store
/// ([`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::platform_branch_named`])
/// resolve a version's branch the same way. A branch name is unique within the
/// catalog, so at most one answers.
pub fn platform_branch_named<'a>(
    branches: &'a BTreeMap<String, PlatformBranch>,
    branch_name: &str,
) -> Option<&'a PlatformBranch> {
    branches
        .values()
        .find(|branch| branch.branch_name == branch_name)
}

/// The AWS-managed platform versions a request in `region` can list.
///
/// The catalog is the service's own: [`SOLUTION_STACKS`] holds the published
/// version of every platform branch, and `branches` — the branch catalog the
/// store holds and `ListPlatformBranches` reports — supplies that branch's
/// life-cycle state and supported tiers, so the two operations cannot disagree
/// about them. Only the ARNs are regional, so the list is rendered per request
/// rather than copied into the store, where it would go stale against the
/// region the caller signed for.
///
/// Every member the catalog states is projected — the platform's category, the
/// operating system it runs on, the add-ons it supports, its branch and the
/// life-cycle state of version and branch — so a client filtering or displaying
/// on one of them reads the same value `DescribePlatformVersion` reports.
/// `PlatformLifecycleState` is [`PLATFORM_LIFECYCLE_RECOMMENDED`] for every
/// entry because the catalog publishes exactly one version per branch, so each
/// published version IS its branch's recommended one. A stack whose branch the
/// store does not hold is not published: a version of a branch nobody can see is
/// not available to the account either.
pub fn managed_platform_listings(
    region: &str,
    branches: &BTreeMap<String, PlatformBranch>,
) -> Vec<ListedPlatformVersion> {
    SOLUTION_STACKS
        .iter()
        .filter_map(|stack| {
            let branch = platform_branch_named(branches, stack.branch_name)?;
            let operating_system = stack.operating_system();
            Some(ListedPlatformVersion {
                summary: PlatformSummary {
                    platform_arn: Some(stack.platform_arn(region)),
                    platform_owner: Some(MANAGED_PLATFORM_OWNER.to_string()),
                    platform_status: Some(PLATFORM_STATUS_READY.to_string()),
                    platform_version: Some(stack.platform_version.to_string()),
                    platform_branch_name: Some(branch.branch_name.clone()),
                    platform_branch_lifecycle_state: Some(reported_branch_lifecycle_state(
                        &branch.lifecycle_state,
                    )),
                    supported_tier_list: Some(branch.supported_tiers.clone()),
                    platform_lifecycle_state: Some(PLATFORM_LIFECYCLE_RECOMMENDED.to_string()),
                    platform_category: Some(stack.platform_category.to_string()),
                    operating_system_name: operating_system.map(|(name, _)| name.to_string()),
                    operating_system_version: operating_system
                        .and_then(|(_, version)| version)
                        .map(str::to_string),
                    supported_addon_list: Some(managed_supported_addons(&branch.supported_tiers)),
                },
                programming_language_names: stack.programming_language_names(),
            })
        })
        .collect()
}

/// Per-service state for com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// Applications by `ApplicationName` (unique within an account).
    pub applications: BTreeMap<String, Application>,
    /// Application versions by `(ApplicationName, VersionLabel)`.
    pub application_versions: BTreeMap<(String, String), ApplicationVersion>,
    /// Configuration templates (saved configurations) by `(ApplicationName,
    /// TemplateName)` — "this name must be unique per application". Populated by
    /// `CreateConfigurationTemplate`; `DescribeApplications` reports an
    /// application's template names from here rather than from a second list on
    /// the application record.
    pub configuration_templates: BTreeMap<(String, String), ConfigurationTemplate>,
    /// Custom platform versions by `PlatformArn`. Populated by
    /// `CreatePlatformVersion`; a `PlatformArn` no entry here answers to names no
    /// platform this account holds.
    pub platform_versions: BTreeMap<String, PlatformVersion>,
    /// Environments by `EnvironmentId`.
    pub environments: BTreeMap<String, Environment>,
    /// `EnvironmentName` -> `EnvironmentId`, so an operation that takes either
    /// one resolves the same record.
    pub environment_name_index: BTreeMap<String, String>,
    /// The service's event log, OLDEST first: every lifecycle call appends here
    /// and `DescribeEvents` reports from it. Append order is the record's
    /// `sequence`, which breaks a tie between two events the clock dated alike.
    pub events: Vec<Event>,
    /// The managed actions this service has carried out on its environments,
    /// OLDEST first, in the append order the record's `sequence` records:
    /// `DescribeEnvironmentManagedActionHistory` reports one environment's
    /// slice of this log, newest first. See [`ManagedActionRecord`], which also
    /// says why nothing appends to it.
    pub managed_action_history: Vec<ManagedActionRecord>,
    /// The Amazon S3 buckets `CreateStorageLocation` has created, by name — one
    /// per region an account has asked for one in. "If the storage location
    /// already exists, `CreateStorageLocation` still returns the bucket name but
    /// does not create a new bucket", so the call reports the bucket this set
    /// holds rather than minting a second one.
    pub storage_buckets: BTreeSet<String>,
    /// The managed platform branches this account can see, by
    /// [`PlatformBranch::key`]. Service-owned reference data: seeded from
    /// [`MANAGED_PLATFORM_BRANCHES`] when the store is created, so every
    /// platform operation reads one catalog.
    pub platform_branches: BTreeMap<String, PlatformBranch>,
    /// When the managed platform catalog was published into this store — the
    /// `DateCreated`/`DateUpdated` a MANAGED platform version reports.
    ///
    /// The catalog is service-owned reference data seeded at store creation and
    /// never re-published, so one instant dates every published version, it is
    /// the same instant on every read, and no read moves it. A CUSTOM version
    /// carries its own creation and update instants on its record instead.
    pub platform_catalog_published_at: ::aws_smithy_types::DateTime,
    /// The machine images the catalog published for each managed platform
    /// version, by [`SolutionStack::catalog_key`] — what
    /// `DescribePlatformVersion` reports in `CustomAmiList`. Seeded with the
    /// catalog; see [`managed_custom_ami_catalog`].
    pub platform_custom_amis: BTreeMap<String, Vec<CustomAmi>>,
    /// Source of unique `EnvironmentId` values, of version sequence numbers and
    /// of event sequence numbers.
    pub next_id_counter: u64,
}

/// The store a fresh engine starts from: no applications, versions, custom
/// platform versions or environments, and the managed platform catalog already
/// published — the
/// latter is service-owned data no caller creates, so `ListPlatformBranches`
/// reads it rather than writing it on first use.
impl Default for ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    fn default() -> Self {
        Self {
            applications: BTreeMap::new(),
            application_versions: BTreeMap::new(),
            configuration_templates: BTreeMap::new(),
            platform_versions: BTreeMap::new(),
            environments: BTreeMap::new(),
            environment_name_index: BTreeMap::new(),
            events: Vec::new(),
            // No managed action has executed on any environment: see
            // `ManagedActionRecord`.
            managed_action_history: Vec::new(),
            // No storage bucket until `CreateStorageLocation` creates one.
            storage_buckets: BTreeSet::new(),
            platform_branches: managed_platform_branch_catalog(),
            platform_catalog_published_at: now(),
            platform_custom_amis: managed_custom_ami_catalog(),
            next_id_counter: 0,
        }
    }
}

impl ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// A fresh unique `EnvironmentId`, in the `e-<13 lowercase alphanumerics>`
    /// form the real service issues one in.
    ///
    /// The id is OPAQUE — `EnvironmentDescription$EnvironmentId` is documented as
    /// "The ID of this environment" and nothing else — so nothing is derived from
    /// the request: the body is random, the way the service's own is, and
    /// id-shape validation on the returned value sees the form it expects. The
    /// randomness is the `uuid` crate's, reached through the same uuid-backed id
    /// newtype [`new_request_id`] uses (its canonical rendering is lowercase hex,
    /// which is inside the alphabet), and the store is checked so an id it
    /// already holds is never handed out twice.
    pub fn next_environment_id(&mut self) -> String {
        loop {
            let candidate = format!("e-{}", random_id_body(ENVIRONMENT_ID_BODY_LENGTH));
            if !self.environments.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The next value of the shared counter.
    pub fn next_counter(&mut self) -> u64 {
        self.next_id_counter += 1;
        self.next_id_counter
    }

    /// Append one event to the log: what happened, how severe it was, which
    /// request produced it and which resources it is about. The event date is
    /// the server clock at the moment it is recorded.
    ///
    /// Every lifecycle handler records through here, so `DescribeEvents` reads
    /// one log rather than reconstructing events from the resources.
    pub fn record_event(
        &mut self,
        request_id: &str,
        severity: &str,
        message: impl Into<String>,
        subject: EventSubject,
    ) {
        self.record_event_at(request_id, severity, message, subject, now());
    }

    /// As [`Self::record_event`], with the event dated `event_date` rather than
    /// the moment of the call: a change the CLOCK owed is dated when it came
    /// due, not when a later call happened to observe it.
    pub fn record_event_at(
        &mut self,
        request_id: &str,
        severity: &str,
        message: impl Into<String>,
        subject: EventSubject,
        event_date: ::aws_smithy_types::DateTime,
    ) {
        let sequence = self.next_counter();
        self.events.push(Event {
            sequence,
            event_date,
            message: message.into(),
            severity: severity.to_string(),
            request_id: request_id.to_string(),
            subject,
        });
        self.evict_retired_events();
    }

    /// Record the events ONE call's activity consists of.
    ///
    /// Elastic Beanstalk's event log reports the ACTIVITY of a call, not just
    /// its outcome: a call records that the operation is starting, then what it
    /// changed, then that the operation completed, and every one of those events
    /// carries the same `RequestId` — which is what lets a client follow one
    /// operation by filtering the log on it. `DescribeEvents` then reports them
    /// newest-first, so the completion is read before the start.
    ///
    /// `operation_name` is the operation as the service names it in those two
    /// messages (lower camel case: `createApplication`). `resource_event` is the
    /// message naming the change itself, for a call that has one; a call whose
    /// own resource message this emulator cannot name records only the two
    /// bookends rather than inventing a third.
    ///
    /// A call whose work the CLOCK finishes later (an environment launch, an
    /// application deletion) does not use this: it records its start here and
    /// its completion when the change comes due. See
    /// [`Self::record_activity_start`].
    pub fn record_completed_activity(
        &mut self,
        request_id: &str,
        operation_name: &str,
        resource_event: Option<String>,
        subject: EventSubject,
    ) {
        self.record_activity_start(request_id, operation_name, subject.clone());
        if let Some(message) = resource_event {
            self.record_event(request_id, SEVERITY_INFO, message, subject.clone());
        }
        self.record_event(
            request_id,
            SEVERITY_INFO,
            format!("{operation_name} completed successfully."),
            subject,
        );
    }

    /// Record that `operation_name`'s activity has STARTED, the first event of
    /// the sequence [`Self::record_completed_activity`] describes.
    pub fn record_activity_start(
        &mut self,
        request_id: &str,
        operation_name: &str,
        subject: EventSubject,
    ) {
        self.record_event(
            request_id,
            SEVERITY_INFO,
            format!("{operation_name} is starting."),
            subject,
        );
    }

    /// One environment's completed and failed managed actions, NEWEST first:
    /// the order `DescribeEnvironmentManagedActionHistory` reports and pages
    /// through them in.
    ///
    /// Newest by the instant the action FINISHED — an action is in the history
    /// because it finished — with the append `sequence` breaking a tie so the
    /// order is total even for two actions that finished inside one clock tick,
    /// and so a page boundary falls in the same place on every call. The
    /// environment is selected by id, so the history of an environment whose
    /// name was later taken by another one is never reported for that other.
    pub fn managed_action_history_of(&self, environment_id: &str) -> Vec<&ManagedActionRecord> {
        let mut history: Vec<&ManagedActionRecord> = self
            .managed_action_history
            .iter()
            .filter(|record| record.environment_id == environment_id)
            .collect();
        history.sort_by(|left, right| {
            right
                .history_instant()
                .cmp(&left.history_instant())
                .then_with(|| right.sequence.cmp(&left.sequence))
        });
        history
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

    /// An application's configuration template names, in the one order
    /// `DescribeApplications` reports them: alphabetically, which is the store's
    /// own key order for the application's slice of the template map.
    pub fn template_names(&self, application_name: &str) -> Vec<String> {
        self.configuration_templates
            .values()
            .filter(|template| template.application_name == application_name)
            .map(|template| template.template_name.clone())
            .collect()
    }

    /// How many configuration templates this account holds — the count the
    /// configuration-template quota covers.
    pub fn configuration_template_count(&self) -> usize {
        self.configuration_templates.len()
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

    /// The id of the environment named by `EnvironmentId` or `EnvironmentName`,
    /// or why no environment answers to the pair.
    ///
    /// Every operation that takes that pair resolves it through here, so they
    /// agree on what "no such environment" means and word it identically; each
    /// one maps [`EnvironmentRefusal::NoIdentifier`] to the code its own model
    /// documents.
    pub fn environment_id_for(
        &self,
        environment_id: Option<&str>,
        environment_name: Option<&str>,
    ) -> Result<String, EnvironmentRefusal> {
        match (environment_id, environment_name) {
            (None, None) => Err(EnvironmentRefusal::NoIdentifier),
            (id, name) => self.resolve_environment_id(id, name).ok_or_else(|| {
                EnvironmentRefusal::NotFound(missing_environment_message(id, name))
            }),
        }
    }

    /// The record `arn` names, for the two tag operations.
    ///
    /// The ARN forms are this service's own ([`application_arn`],
    /// [`application_version_arn`], [`configuration_template_arn`],
    /// [`environment_arn`]), so the same ARN a
    /// create answered with resolves back to the record it minted. The resource
    /// path after the type is matched against the names the store HOLDS rather
    /// than split on `/`, because an application name, a version label and an
    /// environment name may each contain that character, which would otherwise
    /// make the ARN of a stored resource unresolvable. A terminated environment
    /// is not resolved: its resources are gone, so there is nothing left to tag.
    pub fn resolve_tagged_resource(
        &self,
        arn: &str,
    ) -> Result<TaggedResource, ResourceArnRefusal> {
        let Some(resource) = elasticbeanstalk_arn_resource(arn) else {
            return Err(ResourceArnRefusal::NotThisService);
        };
        let (kind, path) = resource.split_once('/').unwrap_or((resource, ""));
        match kind {
            "application" if self.applications.contains_key(path) => {
                Ok(TaggedResource::Application(path.to_string()))
            }
            "applicationversion" => self
                .application_versions
                .keys()
                .find(|(application_name, version_label)| {
                    format!("{application_name}/{version_label}") == path
                })
                .map(|(application_name, version_label)| {
                    TaggedResource::ApplicationVersion(
                        application_name.clone(),
                        version_label.clone(),
                    )
                })
                .ok_or(ResourceArnRefusal::NotFound),
            "configurationtemplate" => self
                .configuration_templates
                .keys()
                .find(|(application_name, template_name)| {
                    format!("{application_name}/{template_name}") == path
                })
                .map(|(application_name, template_name)| {
                    TaggedResource::ConfigurationTemplate(
                        application_name.clone(),
                        template_name.clone(),
                    )
                })
                .ok_or(ResourceArnRefusal::NotFound),
            "environment" => self
                .environments
                .values()
                .find(|environment| {
                    !environment.is_terminated()
                        && format!(
                            "{}/{}",
                            environment.application_name, environment.environment_name
                        ) == path
                })
                .map(|environment| {
                    TaggedResource::Environment(environment.environment_id.clone())
                })
                .ok_or(ResourceArnRefusal::NotFound),
            // A taggable type whose record this store does not hold: the ARN is
            // this service's and its type is one Elastic Beanstalk tags, so what
            // is missing is the resource.
            kind if TAGGABLE_RESOURCE_TYPES.contains(&kind) => {
                Err(ResourceArnRefusal::NotFound)
            }
            _ => Err(ResourceArnRefusal::UnsupportedType),
        }
    }

    /// The tags stored on `resource`.
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<&Vec<Tag>> {
        match resource {
            TaggedResource::Application(name) => {
                self.applications.get(name).map(|record| &record.tags)
            }
            TaggedResource::ApplicationVersion(application_name, version_label) => self
                .application_versions
                .get(&(application_name.clone(), version_label.clone()))
                .map(|record| &record.tags),
            TaggedResource::ConfigurationTemplate(application_name, template_name) => self
                .configuration_templates
                .get(&(application_name.clone(), template_name.clone()))
                .map(|record| &record.tags),
            TaggedResource::Environment(environment_id) => {
                self.environments.get(environment_id).map(|record| &record.tags)
            }
        }
    }

    /// The tags stored on `resource`, for a tag mutation to write through.
    pub fn tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<Tag>> {
        match resource {
            TaggedResource::Application(name) => {
                self.applications.get_mut(name).map(|record| &mut record.tags)
            }
            TaggedResource::ApplicationVersion(application_name, version_label) => self
                .application_versions
                .get_mut(&(application_name.clone(), version_label.clone()))
                .map(|record| &mut record.tags),
            TaggedResource::ConfigurationTemplate(application_name, template_name) => self
                .configuration_templates
                .get_mut(&(application_name.clone(), template_name.clone()))
                .map(|record| &mut record.tags),
            TaggedResource::Environment(environment_id) => self
                .environments
                .get_mut(environment_id)
                .map(|record| &mut record.tags),
        }
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

    /// The published branch named `branch_name`, if the catalog holds it.
    ///
    /// A platform version's branch attributes (its supported tiers, its branch
    /// life-cycle state) are held once, on the branch, so every operation that
    /// reports them for a version resolves the branch through here.
    pub fn platform_branch_named(&self, branch_name: &str) -> Option<&PlatformBranch> {
        platform_branch_named(&self.platform_branches, branch_name)
    }

    /// How many environments have not reached `Terminated` — the count the
    /// environment quota covers.
    pub fn running_environment_count(&self) -> usize {
        self.environments
            .values()
            .filter(|environment| !environment.is_terminated())
            .count()
    }

    /// Materialise every change the clock has already made due — an
    /// environment's status, a platform version's status, and an application
    /// whose deletion has completed — record the event each completed change
    /// owes, and drop the events the clock has carried out of the reporting
    /// window. Every handler that reads or acts on one of them calls this first,
    /// so no two of them disagree about what state the store is in.
    pub fn materialize_due_transitions(&mut self) {
        let at = Instant::now();
        self.materialize_due_application_deletions(at);
        // The event cannot be appended while `environments` is borrowed, so the
        // completions are collected first and recorded below.
        let mut completed: Vec<CompletedTransition> = Vec::new();
        for environment in self.environments.values_mut() {
            if let Some(transition) = environment.materialize_due(at) {
                completed.push(transition);
            }
        }
        for transition in completed {
            self.record_event_at(
                &transition.request_id,
                SEVERITY_INFO,
                transition.message,
                transition.subject,
                transition.at,
            );
        }
        self.expire_events();
        for platform_version in self.platform_versions.values_mut() {
            platform_version.materialize_due(at);
        }
        self.evict_retired_environments();
    }

    /// Materialise just the application deletions the clock has already
    /// completed — the part of [`Self::materialize_due_transitions`] that an
    /// operation acting on an APPLICATION owes before it reads the application
    /// store, so a name whose deletion is already due is not reported as taken
    /// and a version is not created inside an application the clock has
    /// removed. An operation that touches an environment or a platform version
    /// calls the whole of it instead.
    pub fn materialize_due_application_removals(&mut self) {
        self.materialize_due_application_deletions(Instant::now());
    }

    /// Remove every application whose deletion the clock has already completed,
    /// recording the event that completion owes.
    ///
    /// The removal was due at `due_at` whether or not anybody called, so
    /// materialising it on a read cannot hide an application that was still
    /// describable: until then the record is reported exactly as it stands, the
    /// way the real service reports an application whose deletion is still
    /// running.
    fn materialize_due_application_deletions(&mut self, at: Instant) {
        let due: Vec<(String, PendingDeletion)> = self
            .applications
            .values()
            .filter_map(|application| {
                let pending = application.pending_deletion.clone()?;
                (pending.due_at <= at).then(|| (application.name.clone(), pending))
            })
            .collect();
        for (application_name, pending) in due {
            self.applications.remove(&application_name);
            self.record_event_at(
                &pending.request_id,
                SEVERITY_INFO,
                "deleteApplication completed successfully.",
                EventSubject {
                    application_name: Some(application_name),
                    ..Default::default()
                },
                pending.due_date,
            );
        }
    }

    /// Drop every event older than [`EVENT_RETENTION`] — the window
    /// `DescribeEvents` reports over. The expiry was due at the moment the event
    /// aged out, whether or not anybody called, so applying it here cannot hide
    /// an event that was still reportable.
    fn expire_events(&mut self) {
        // The horizon as an EXACT instant: the retention taken off the clock's
        // own (seconds, nanoseconds) rather than off a float of epoch seconds,
        // whose step at the current epoch is coarser than the nanoseconds
        // [`instant_key`] compares — the same reason the comparison below goes
        // through that key.
        let (now_secs, now_nanos) = instant_key(&now());
        let retention_secs = i64::try_from(EVENT_RETENTION.as_secs()).unwrap_or(i64::MAX);
        let mut secs = now_secs.saturating_sub(retention_secs);
        let mut nanos = i64::from(now_nanos) - i64::from(EVENT_RETENTION.subsec_nanos());
        if nanos < 0 {
            // Borrow a whole second, so the horizon stays a normalised instant.
            nanos += 1_000_000_000;
            secs = secs.saturating_sub(1);
        }
        let horizon = (secs, u32::try_from(nanos).unwrap_or_default());
        self.events
            .retain(|event| instant_key(&event.event_date) >= horizon);
    }

    /// Hold the count bound on the event log: past [`MAX_RETAINED_EVENTS`] the
    /// oldest-DATED events are dropped, so a create/terminate loop cannot grow
    /// the log without bound inside a single run, where nothing is six weeks old
    /// yet.
    ///
    /// By date, not by append position: a clock-owed completion is recorded with
    /// the date it came due, so it can sit later in the append order than its
    /// date. Dropping by date is what makes the bound agree with the order
    /// `DescribeEvents` reports in, so the record dropped is always one that had
    /// already fallen off the end of the answer.
    fn evict_retired_events(&mut self) {
        while self.events.len() > MAX_RETAINED_EVENTS {
            let oldest = self
                .events
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| {
                    instant_key(&left.event_date)
                        .cmp(&instant_key(&right.event_date))
                        .then_with(|| left.sequence.cmp(&right.sequence))
                })
                .map(|(index, _)| index);
            match oldest {
                Some(index) => {
                    self.events.remove(index);
                }
                None => break,
            }
        }
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
/// request: it names the member the caller used and the value it carried —
/// `No Environment found for EnvironmentName = '<name>'.`
///
/// This is the wording the real service answers MOST of its environment lookups
/// in (`RestartAppServer`, `RebuildEnvironment`, `AbortEnvironmentUpdate`, the
/// two operations-role calls, `ApplyEnvironmentManagedAction` and the two health
/// reads, as their recorded transcripts show). The environment-lifecycle pair
/// answers in a template of its own, which is
/// [`missing_environment_in_account_message`]: an operation's not-found message
/// is the one the real service words THAT operation's refusal with, so neither
/// template is applied to an operation that does not answer in it.
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

/// The role a request gives the environment it names, for an operation that
/// names MORE than one: `SwapEnvironmentCNAMEs` takes a source and a
/// destination, and the real service leads the not-found message with that role
/// ("source environment not found with Name: …") so a caller can tell which of
/// the two inputs was bad. See [`missing_environment_in_account_message`].
pub const ENVIRONMENT_ROLE_SOURCE: &str = "source";
/// The destination half of [`ENVIRONMENT_ROLE_SOURCE`].
pub const ENVIRONMENT_ROLE_DESTINATION: &str = "destination";

/// The message the ENVIRONMENT-LIFECYCLE pair — `TerminateEnvironment` and
/// `SwapEnvironmentCNAMEs` — refuses a request with `InvalidParameterValue` for
/// when the identifier it supplied names no stored environment.
///
/// Also built from the request alone, but in the real service's other template
/// for these two: the kind of identifier the caller used (`Id` or `Name`), the
/// value it carried, and the ACCOUNT the lookup was made in —
/// `Environment not found with Name: <value> in account '<account>'`. `role`
/// (`None` for an operation that names a single environment) replaces the
/// leading `Environment` with that environment's role, lower case, the way the
/// real service words the two sides of a swap. Shared by the operations that
/// answer in it, so they cannot disagree; every other environment lookup answers
/// in [`missing_environment_message`].
pub fn missing_environment_in_account_message(
    role: Option<&str>,
    environment_id: Option<&str>,
    environment_name: Option<&str>,
) -> String {
    let subject = match role {
        Some(role) => format!("{role} environment"),
        None => "Environment".to_string(),
    };
    let account = ::engine::emulator_account_id();
    match (environment_id, environment_name) {
        (Some(id), _) => format!("{subject} not found with Id: {id} in account '{account}'"),
        (None, Some(name)) => {
            format!("{subject} not found with Name: {name} in account '{account}'")
        }
        // Neither supplied is a MissingRequiredParameter refusal, which every
        // caller makes before reaching here.
        (None, None) => format!("{subject} not found in account '{account}'"),
    }
}

/// The message `DescribePlatformVersion` refuses a `PlatformArn` that answers to
/// no platform version with — neither a CUSTOM one this account created nor a
/// MANAGED one this service publishes.
///
/// Built from the request, in the real service's template for the platform
/// READ: the requested ARN quoted, then `does not exist.`. `CreateEnvironment`
/// words its own platform refusal differently ("No Platform named '<arn>'
/// found."), the same per-operation split the two environment templates above
/// have, so this one is not applied there.
pub fn missing_platform_message(platform_arn: &str) -> String {
    format!("Platform '{platform_arn}' does not exist.")
}

/// The message that records a created resource, the middle event of the
/// sequence [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::record_completed_activity`]
/// describes: `Created new <kind>: <identifier>`, the form the real service's
/// `Created new Application: <name>` event uses. [`RESOURCE_APPLICATION`],
/// [`RESOURCE_APPLICATION_VERSION`] and [`RESOURCE_CONFIGURATION_TEMPLATE`] are
/// the kinds this service creates
/// synchronously.
pub fn resource_created_event(kind: &str, identifier: &str) -> String {
    format!("Created new {kind}: {identifier}")
}

/// The resource kind an application's created-event names. See
/// [`resource_created_event`].
pub const RESOURCE_APPLICATION: &str = "Application";
/// The resource kind an application version's created-event names. See
/// [`resource_created_event`].
pub const RESOURCE_APPLICATION_VERSION: &str = "Application Version";
/// The resource kind a configuration template's created-event names. See
/// [`resource_created_event`].
pub const RESOURCE_CONFIGURATION_TEMPLATE: &str = "Configuration Template";

/// The message this service refuses a request with `InvalidParameterValue` for
/// when the `(ApplicationName, TemplateName)` pair it supplied names no stored
/// configuration template, built from the request: `No Configuration Template
/// named '<name>' found.`
///
/// The same shape as the not-found sentences the create operations answer a bad
/// `ApplicationName` or `VersionLabel` with (`No Application named '<name>'
/// found.`), read from the one place every caller reads it, so two operations
/// cannot word one refusal two ways.
pub fn missing_configuration_template_message(template_name: &str) -> String {
    format!("No Configuration Template named '{template_name}' found.")
}

/// How many characters an `EnvironmentId` body carries: `e-` plus 13 lowercase
/// alphanumerics is the form the real service issues one in.
pub const ENVIRONMENT_ID_BODY_LENGTH: usize = 13;

/// `length` random lowercase hexadecimal characters, for an opaque
/// server-generated id.
///
/// Taken from the canonical rendering of v4 UUIDs — lowercase hex, which is
/// inside the lowercase-alphanumeric alphabet an id body uses — drawn through
/// the same uuid-backed id newtype [`new_request_id`] uses, so this crate
/// hand-rolls no generator and reaches the `uuid` crate by one path only. As
/// many UUIDs are drawn as the length needs; 13 hex characters carry 52 bits,
/// which is what makes a collision (checked for anyway) a non-event.
fn random_id_body(length: usize) -> String {
    let mut body = String::with_capacity(length);
    while body.len() < length {
        let drawn = ::emulator_telemetry::BatchId::random().as_str().to_string();
        body.extend(
            drawn
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .take(length - body.len()),
        );
    }
    body
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

// ---------------------------------------------------------------------------
// The CNAME reservation rule
//
// A CNAME ([`environment_cname`]) is held by at most ONE live environment, and
// a prefix that cannot be a host name label names no CNAME at all. Both halves
// live here, together, because `CheckDNSAvailability` reports on them and
// `CreateEnvironment` enforces them, and the two must not disagree.
// ---------------------------------------------------------------------------

/// Whether `prefix` can be the leftmost label of an environment CNAME at all.
///
/// A CNAME prefix is a host name label, so it carries the label syntax every
/// host name obeys (RFC 1123 2.1): ASCII letters, digits and hyphens only, and
/// no leading or trailing hyphen. The model's own `CreateEnvironment$
/// EnvironmentName` doc states the same rule for the value that becomes the
/// prefix when the caller supplies none — "The name can contain only letters,
/// numbers, and hyphens. It can't start or end with a hyphen." The 4..63 length
/// bound is the `DNSCnamePrefix` length trait, which the boundary validator has
/// already refused outside of, so it is not re-checked here.
pub fn is_legal_cname_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix.starts_with('-')
        && !prefix.ends_with('-')
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// The live environment in `store` that holds `cname`, if any — the one record
/// that has that DNS name reserved. A terminated environment holds nothing, so
/// its CNAME is free again. DNS names are case-insensitive, so the comparison
/// is too.
pub fn environment_holding_cname<'store>(
    store: &'store ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    cname: &str,
) -> Option<&'store Environment> {
    store.environments.values().find(|environment| {
        !environment.is_terminated() && environment.cname.eq_ignore_ascii_case(cname)
    })
}
