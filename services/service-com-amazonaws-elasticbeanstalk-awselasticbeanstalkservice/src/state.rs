// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice service.
//!
//! This is the single store every handler in `src/ops/` reads and writes: the
//! applications, the application versions, the custom platform versions and the
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
    ApplicationVersionLifecycleConfig, ConfigurationOptionSetting, EnvironmentTier,
    EventDescription, MaxAgeRule, MaxCountRule, PlatformBranchSummary, PlatformDescription,
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
/// DescribeAccountAttributes as `ConfigurationTemplateQuota`;
/// `CreateConfigurationTemplate` is still scaffolded, so this is the allowance
/// that create will be held to rather than one anything enforces yet.
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

    /// The modelled `PlatformDescription` for this published version.
    ///
    /// What the catalog states is reported; what only a built platform's
    /// definition carries (its category, maintainer, operating system, languages,
    /// frameworks, creation dates) is left absent rather than guessed.
    pub fn to_description(&self, region: &str) -> PlatformDescription {
        PlatformDescription {
            platform_arn: Some(self.platform_arn(region)),
            platform_owner: Some(MANAGED_PLATFORM_OWNER.to_string()),
            platform_name: Some(self.branch_name.to_string()),
            platform_version: Some(self.platform_version.to_string()),
            platform_branch_name: Some(self.branch_name.to_string()),
            solution_stack_name: Some(self.name()),
            // A published version is usable as it stands; nothing is building it.
            platform_status: Some(PLATFORM_STATUS_READY.to_string()),
            date_created: None,
            date_updated: None,
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
            platform_branch_lifecycle_state: None,
        }
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
    },
    SolutionStack {
        branch_name: ".NET 8 running on 64bit Amazon Linux 2023",
        platform_version: "3.3.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: ".NET Core running on 64bit Amazon Linux 2",
        platform_version: "2.8.0",
        permitted_file_types: &["zip"],
    },
    // .NET on Windows Server.
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2019",
        platform_version: "2.16.0",
        permitted_file_types: &["zip"],
    },
    // Docker.
    SolutionStack {
        branch_name: "Docker running on 64bit Amazon Linux 2023",
        platform_version: "4.2.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "Docker running on 64bit Amazon Linux 2",
        platform_version: "3.9.0",
        permitted_file_types: &["zip"],
    },
    // Go.
    SolutionStack {
        branch_name: "Go 1 running on 64bit Amazon Linux 2023",
        platform_version: "3.5.0",
        permitted_file_types: &["zip"],
    },
    // Java SE (Corretto): a Java SE platform deploys a `jar`.
    SolutionStack {
        branch_name: "Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: "4.4.0",
        permitted_file_types: &["jar", "zip"],
    },
    SolutionStack {
        branch_name: "Corretto 17 running on 64bit Amazon Linux 2023",
        platform_version: "4.3.0",
        permitted_file_types: &["jar", "zip"],
    },
    SolutionStack {
        branch_name: "Corretto 11 running on 64bit Amazon Linux 2",
        platform_version: "3.7.0",
        permitted_file_types: &["jar", "zip"],
    },
    // Node.js.
    SolutionStack {
        branch_name: "Node.js 22 running on 64bit Amazon Linux 2023",
        platform_version: "6.6.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "Node.js 20 running on 64bit Amazon Linux 2023",
        platform_version: "6.5.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "Node.js 18 running on 64bit Amazon Linux 2023",
        platform_version: "6.4.0",
        permitted_file_types: &["zip"],
    },
    // PHP.
    SolutionStack {
        branch_name: "PHP 8.4 running on 64bit Amazon Linux 2023",
        platform_version: "4.5.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "PHP 8.3 running on 64bit Amazon Linux 2023",
        platform_version: "4.4.0",
        permitted_file_types: &["zip"],
    },
    // Python.
    SolutionStack {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        platform_version: "4.6.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "Python 3.11 running on 64bit Amazon Linux 2023",
        platform_version: "4.5.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "Python 3.9 running on 64bit Amazon Linux 2023",
        platform_version: "4.4.0",
        permitted_file_types: &["zip"],
    },
    // Ruby.
    SolutionStack {
        branch_name: "Ruby 3.4 running on 64bit Amazon Linux 2023",
        platform_version: "3.5.0",
        permitted_file_types: &["zip"],
    },
    SolutionStack {
        branch_name: "Ruby 3.2 running on 64bit Amazon Linux 2023",
        platform_version: "3.4.0",
        permitted_file_types: &["zip"],
    },
    // Tomcat: a Tomcat platform deploys a `war`.
    SolutionStack {
        branch_name: "Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: "5.4.0",
        permitted_file_types: &["war", "zip"],
    },
    SolutionStack {
        branch_name: "Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023",
        platform_version: "5.3.0",
        permitted_file_types: &["war", "zip"],
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
    /// Template names, carried so DescribeApplications reports them; creating a
    /// configuration template is its own (still scaffolded) operation.
    pub configuration_templates: Vec<String>,
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
            // derive reports no value rather than a guessed one.
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
        self.date_updated = pending.due_date;
        if self.status == STATUS_TERMINATED {
            self.date_terminated = Some(pending.due_date);
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

/// `PlatformLifecycleState` for the newest version of a branch, as
/// `PlatformSummary$PlatformLifecycleState` documents it: "Possible values:
/// `recommended` | empty. If an empty value is returned, the platform version is
/// supported but isn't the recommended one for its branch."
pub const PLATFORM_LIFECYCLE_RECOMMENDED: &str = "recommended";

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

/// The AWS-managed platform versions a request in `region` can list, as the
/// modelled `PlatformSummary`.
///
/// The catalog is the service's own: [`SOLUTION_STACKS`] holds the published
/// version of every platform branch, and `branches` — the branch catalog the
/// store holds and `ListPlatformBranches` reports — supplies that branch's
/// life-cycle state and supported tiers, so the two operations cannot disagree
/// about them. Only the ARNs are regional, so the list is rendered per request
/// rather than copied into the store, where it would go stale against the
/// region the caller signed for.
///
/// `PlatformLifecycleState` is [`PLATFORM_LIFECYCLE_RECOMMENDED`] for every
/// entry because the catalog publishes exactly one version per branch, so each
/// published version IS its branch's recommended one. A stack whose branch the
/// store does not hold is not published: a version of a branch nobody can see is
/// not available to the account either. What only a built platform's definition
/// archive carries (its category, operating system and addons) is left ABSENT
/// rather than guessed, exactly as [`SolutionStack::to_description`] leaves it.
pub fn managed_platform_summaries(
    region: &str,
    branches: &BTreeMap<String, PlatformBranch>,
) -> Vec<PlatformSummary> {
    SOLUTION_STACKS
        .iter()
        .filter_map(|stack| {
            let branch = branches
                .values()
                .find(|branch| branch.branch_name == stack.branch_name)?;
            Some(PlatformSummary {
                platform_arn: Some(stack.platform_arn(region)),
                platform_owner: Some(MANAGED_PLATFORM_OWNER.to_string()),
                platform_status: Some(PLATFORM_STATUS_READY.to_string()),
                platform_version: Some(stack.platform_version.to_string()),
                platform_branch_name: Some(branch.branch_name.clone()),
                platform_branch_lifecycle_state: Some(branch.lifecycle_state.clone()),
                supported_tier_list: Some(branch.supported_tiers.clone()),
                platform_lifecycle_state: Some(PLATFORM_LIFECYCLE_RECOMMENDED.to_string()),
                platform_category: None,
                operating_system_name: None,
                operating_system_version: None,
                supported_addon_list: None,
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
            platform_versions: BTreeMap::new(),
            environments: BTreeMap::new(),
            environment_name_index: BTreeMap::new(),
            events: Vec::new(),
            // No storage bucket until `CreateStorageLocation` creates one.
            storage_buckets: BTreeSet::new(),
            platform_branches: managed_platform_branch_catalog(),
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
    /// [`application_version_arn`], [`environment_arn`]), so the same ARN a
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
        let horizon = now().as_secs_f64() - EVENT_RETENTION.as_secs_f64();
        self.events
            .retain(|event| event.event_date.as_secs_f64() >= horizon);
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
                    left.event_date
                        .as_secs_f64()
                        .total_cmp(&right.event_date.as_secs_f64())
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

/// The message that records a created resource, the middle event of the
/// sequence [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::record_completed_activity`]
/// describes: `Created new <kind>: <identifier>`, the form the real service's
/// `Created new Application: <name>` event uses. [`RESOURCE_APPLICATION`] and
/// [`RESOURCE_APPLICATION_VERSION`] are the kinds this service creates
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
