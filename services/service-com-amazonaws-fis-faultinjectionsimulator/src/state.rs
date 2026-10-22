// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-fis-faultinjectionsimulator service.
//!
//! AWS FIS has two caller-owned resources, and this module holds both:
//!
//! * an EXPERIMENT TEMPLATE, created by `CreateExperimentTemplate`, read back by
//!   `GetExperimentTemplate`, enumerated by `ListExperimentTemplates` and removed by
//!   `DeleteExperimentTemplate`;
//! * an EXPERIMENT, minted by `StartExperiment` from a template, read back by
//!   `GetExperiment`, enumerated by `ListExperiments` and stopped by `StopExperiment`.
//!
//! An experiment template also owns a sub-resource: a TARGET ACCOUNT CONFIGURATION per
//! targeted account, created by `CreateTargetAccountConfiguration`, read back by
//! `GetTargetAccountConfiguration` and enumerated by `ListTargetAccountConfigurations`. It
//! is named by the (template, account) pair, so it is held on the template record and its
//! count is derived from that map.
//!
//! An experiment carries one more derived record: the RESOLVED TARGETS its targets
//! resolved to when it was started ([`resolve_targets`]), enumerated by
//! `ListExperimentResolvedTargets`. It is computed once, at start, and held on the
//! experiment record, so every read of it reports the resolution that start computed.
//!
//! Besides those caller-owned records it holds the service's own data: the built-in FIS
//! ACTION catalog ([`ACTION_CATALOG`]), which `GetAction` reports from. No API creates,
//! updates or deletes an action, so that catalog is a constant table rather than a map a
//! handler writes.
//!
//! Both records are the one copy every handler reads and writes, so a value a create
//! call returns is the value a later read reports. An experiment also owes the clock a
//! transition (`initiating` -> `running`, `stopping` -> `stopped`): the instant it falls
//! due is stored on the record and materialised by [`ExperimentRecord::settle`], so no
//! handler ever sleeps and a read never invents a status the clock did not already owe.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Action, ActionParameter, ActionTarget, ConflictException, CreateExperimentTemplateActionInput,
    CreateExperimentTemplateExperimentOptionsInput, CreateExperimentTemplateInput,
    CreateExperimentTemplateLogConfigurationInput,
    CreateExperimentTemplateReportConfigurationInput, CreateExperimentTemplateStopConditionInput,
    CreateExperimentTemplateTargetInput, Experiment, ExperimentAction,
    ExperimentCloudWatchLogsLogConfiguration, ExperimentLogConfiguration, ExperimentOptions,
    ExperimentReportConfiguration, ExperimentReportConfigurationCloudWatchDashboard,
    ExperimentReportConfigurationDataSources, ExperimentReportConfigurationOutputs,
    ExperimentReportConfigurationOutputsS3Configuration, ExperimentS3LogConfiguration,
    ExperimentState, ExperimentStopCondition, ExperimentSummary, ExperimentTarget,
    ExperimentTargetAccountConfiguration, ExperimentTargetAccountConfigurationSummary,
    ExperimentTargetFilter, ExperimentTemplate,
    ExperimentTemplateAction,
    ExperimentTemplateCloudWatchLogsLogConfiguration,
    ExperimentTemplateCloudWatchLogsLogConfigurationInput, ExperimentTemplateExperimentOptions,
    ExperimentTemplateLogConfiguration, ExperimentTemplateReportConfiguration,
    ExperimentTemplateReportConfigurationCloudWatchDashboard,
    ExperimentTemplateReportConfigurationDataSources,
    ExperimentTemplateReportConfigurationDataSourcesInput,
    ExperimentTemplateReportConfigurationOutputs,
    ExperimentTemplateReportConfigurationOutputsInput, ExperimentTemplateS3LogConfiguration,
    ExperimentTemplateS3LogConfigurationInput, ExperimentTemplateStopCondition,
    ExperimentTemplateSummary, ExperimentTemplateTarget, ExperimentTemplateTargetFilter,
    ExperimentTemplateTargetInputFilter, ReportConfigurationCloudWatchDashboardInput,
    ReportConfigurationS3Output, ReportConfigurationS3OutputInput, ResolvedTarget,
    ResourceNotFoundException,
    ServiceQuotaExceededException, TargetAccountConfiguration, TargetAccountConfigurationSummary,
    ValidationException,
    ACCOUNT_TARGETING_VALUES, ACTIONS_MODE_VALUES, EMPTY_TARGET_RESOLUTION_MODE_VALUES,
};

/// What a handler reports when the engine it was built on carries no store for this
/// service — a server fault, never a bad request.
pub const STORE_UNAVAILABLE: &str = "the fis experiment store is unavailable";

/// The region an ARN names when the request carries no SigV4 credential scope to read
/// one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `arnNamespace` / `sigv4Name` — the third field of every ARN it mints.
pub const ARN_SERVICE: &str = "fis";

/// The ARN resource prefix of an experiment template
/// (`arn:aws:fis:<region>:<account>:experiment-template/<id>`).
pub const TEMPLATE_RESOURCE: &str = "experiment-template";

/// The ARN resource prefix of an experiment
/// (`arn:aws:fis:<region>:<account>:experiment/<id>`).
pub const EXPERIMENT_RESOURCE: &str = "experiment";

/// The prefix of a minted experiment-template id, as FIS writes it (`EXT…`).
pub const TEMPLATE_ID_PREFIX: &str = "EXT";

/// The prefix of a minted experiment id, as FIS writes it (`EXP…`).
pub const EXPERIMENT_ID_PREFIX: &str = "EXP";

/// The ARN resource prefix of a FIS action (`arn:aws:fis:<region>::action/<id>`).
///
/// Unlike an experiment or an experiment template, an action is SERVICE-owned: no API
/// creates one, so its ARN carries no account segment. The AWS Service Authorization
/// Reference gives the action resource ARN as
/// `arn:${Partition}:fis:${Region}::action/${Id}`.
pub const ACTION_RESOURCE: &str = "action";

/// How many characters follow the prefix in a minted id.
///
/// `ExperimentTemplateId` and `ExperimentId` are modelled `length(0..64)`,
/// `pattern ^[\S]+$`, which admits any short token; FIS itself writes a short
/// alphanumeric suffix, and twelve base-36 characters is far more than this emulator can
/// exhaust.
pub const ID_SUFFIX_LEN: usize = 12;

/// The largest page a paginated FIS list serves, and the page size used when a request
/// names no `maxResults`: every `maxResults` member of this service
/// (`ListExperimentTemplates`, `ListExperiments`, `ListTargetAccountConfigurations`) is
/// modelled `@range(min: 1, max: 100)`.
pub const PAGE_LIMIT: i32 = 100;

/// How many experiment templates the store holds before `CreateExperimentTemplate`
/// refuses with `ServiceQuotaExceededException`.
///
/// This is the emulator's OWN bound on its store rather than a claim about any quota AWS
/// publishes: without it a caller could mint records without end. It is set far above any
/// workflow an emulator serves, and it is checked only when a request would ADD a record.
pub const MAX_EXPERIMENT_TEMPLATES: usize = 1_000;

/// How many experiments the store holds before `StartExperiment` refuses with
/// `ServiceQuotaExceededException`. See [`MAX_EXPERIMENT_TEMPLATES`].
pub const MAX_EXPERIMENTS: usize = 1_000;

/// How many target account configurations ONE experiment template holds before
/// `CreateTargetAccountConfiguration` refuses with `ServiceQuotaExceededException`.
///
/// Like [`MAX_EXPERIMENT_TEMPLATES`] this is the emulator's OWN bound on its store
/// rather than a claim about a quota AWS publishes, and it is checked only when a
/// request would ADD a configuration.
pub const MAX_TARGET_ACCOUNT_CONFIGURATIONS: usize = 1_000;

/// `ResourceArnList`'s published ceiling (`@length(min: 0, max: 5)`) — the most resources
/// ONE target of an experiment template may name.
///
/// The boundary validator refuses a longer `resourceArns` before any handler runs, so
/// this is the bound on the per-target work [`resolve_targets`] does. It is named here
/// because that bound is what makes the resolution of one target constant-sized.
pub const MAX_TARGET_RESOURCE_ARNS: usize = 5;

/// How many resolved targets ONE experiment's resolution holds.
///
/// The ceiling [`resolve_targets`] resolves up to: an experiment resolves the first
/// [`MAX_RESOLVED_TARGETS`] resources its targets identify, so neither the work that
/// resolution costs nor the records it leaves in the store grows with how many targets a
/// request put on a template (the model bounds the resources ONE target names, see
/// [`MAX_TARGET_RESOURCE_ARNS`], but not how many targets a template carries). The
/// emulator's OWN bound, like [`MAX_EXPERIMENT_TEMPLATES`], and far above any workflow an
/// emulator serves.
pub const MAX_RESOLVED_TARGETS: usize = 1_000;

/// `TargetAccountId`'s modelled bounds (`@length(min: 12, max: 48)`) — the floor.
///
/// The boundary validator applies them to a modelled `accountId` member; they are named
/// here because [`target_account_page_token_account`] checks them on a `nextToken`, which
/// the model types as a plain `NextToken`.
pub const TARGET_ACCOUNT_ID_FLOOR: usize = 12;

/// See [`TARGET_ACCOUNT_ID_FLOOR`] — the ceiling of `TargetAccountId`'s modelled
/// `@length`.
pub const TARGET_ACCOUNT_ID_CEILING: usize = 48;

/// How many digits an AWS account number has — the account segment of an ARN.
///
/// Named here because [`is_iam_role_arn`] checks it on the account segment of a
/// `roleArn`, which the model types as a plain `RoleArn` string.
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// The message FIS answers for an experiment template reference that resolves to no
/// record, whichever operation resolved it.
///
/// A constant rather than a request-derived string: the identical text is recorded for
/// different (and differently shaped) `experimentTemplateId` values, so the requested
/// identifier does NOT appear in it. `ResourceNotFoundException$message` is modelled only
/// as `ExceptionMessage @length(0..1024)` ("The specified resource cannot be found."), so
/// the text itself is a server-side rule, named here once for every resolver of a
/// template id.
pub const EXPERIMENT_TEMPLATE_NOT_FOUND: &str = "ExperimentTemplate not found.";

/// The message FIS answers for a target account configuration the named experiment
/// TEMPLATE does not hold.
///
/// A constant rather than a request-derived string: the identical text is recorded for
/// different template/account pairs, so neither half of the resource's name appears in
/// it.
pub const TARGET_ACCOUNT_CONFIGURATION_NOT_FOUND: &str = "TargetAccountConfiguration not found.";

/// The message FIS answers for a `nextToken` no call of the list issued.
///
/// Also a constant: the text varies neither with the token nor with the collection being
/// paginated, so every paginated list of this service refuses an unissued token with it.
pub const INVALID_NEXT_TOKEN: &str = "The provided nextToken is invalid.";

/// The marker every `nextToken` a list of target account configurations issues starts
/// with — `ListTargetAccountConfigurations` over a template's configurations and
/// `ListExperimentTargetAccountConfigurations` over an experiment's snapshot of them.
///
/// A target account configuration is named by its `accountId` rather than by an id this
/// service mints, so there is no minted shape to recognise a page token by (the way
/// [`is_minted_id`] recognises the token of a template or experiment page). The token
/// therefore carries the account id the next page resumes at BEHIND this marker, which
/// makes it self-describing: a value these operations never issued carries no marker and
/// is refused as a `ValidationException` instead of being read as a resume point.
pub const TARGET_ACCOUNT_PAGE_TOKEN_PREFIX: &str = "targetAccountConfiguration.";

/// The value FIS applies to `accountTargeting` when the request names none: the
/// experiment targets the caller's own account (`AccountTargeting` is modelled
/// `multi-account | single-account`, and multi-account targeting is what target-account
/// configurations opt into).
pub const DEFAULT_ACCOUNT_TARGETING: &str = "single-account";

/// The value FIS applies to `emptyTargetResolutionMode` when the request names none: an
/// action whose target resolves to nothing fails the experiment rather than being skipped
/// (`EmptyTargetResolutionMode` is modelled `fail | skip`).
pub const DEFAULT_EMPTY_TARGET_RESOLUTION_MODE: &str = "fail";

/// The value FIS applies to `actionsMode` when `StartExperiment` names none: the
/// experiment runs all of its actions (`ActionsMode` is modelled `run-all | skip-all`).
pub const DEFAULT_ACTIONS_MODE: &str = "run-all";

/// The `selectionMode` that keeps every identified resource, which FIS documents as the
/// default: "ALL - Run the action on all identified targets. This is the default."
///
/// `SelectionMode` is modelled as a plain string, so the three forms FIS documents
/// (`ALL`, `COUNT(n)`, `PERCENT(n)`) are spelled out here and read by
/// [`selection_count`].
pub const SELECTION_MODE_ALL: &str = "ALL";

/// The `selectionMode` form that keeps a fixed NUMBER of the identified resources:
/// "COUNT(n) - Run the action on the specified number of targets … For example, COUNT(1)
/// selects one of the targets."
pub const SELECTION_MODE_COUNT_PREFIX: &str = "COUNT(";

/// The `selectionMode` form that keeps a PERCENTAGE of the identified resources:
/// "PERCENT(n) - Run the action on the specified percentage of targets … For example,
/// PERCENT(25) selects 25% of the targets."
pub const SELECTION_MODE_PERCENT_PREFIX: &str = "PERCENT(";

/// The `ResolvedTarget$targetInformation` key the resolved resource's ARN is reported
/// under.
///
/// `TargetInformationMap` is modelled as a free `string -> string` map ("Information about
/// the target") with no documented catalog of keys, and the only thing this emulator KNOWS
/// about a resolved resource is the ARN its target named — so that ARN is what it reports,
/// under this key. Within `TargetInformationKey`'s modelled `@length(0..64)` and
/// `^[\S]+$`.
pub const TARGET_INFORMATION_ARN_KEY: &str = "arn";

/// The marker every `nextToken` `ListExperimentResolvedTargets` issues starts with.
///
/// A resolved target is not named by an id this service mints — it is one entry of an
/// experiment's resolution, so what a page resumes at is its POSITION in that
/// resolution. The token carries that position behind this marker, which makes it
/// self-describing: a value this operation never issued carries no marker and is refused
/// as a `ValidationException` instead of being read as a resume point (the same shape as
/// [`TARGET_ACCOUNT_PAGE_TOKEN_PREFIX`]).
pub const RESOLVED_TARGET_PAGE_TOKEN_PREFIX: &str = "resolvedTarget.";

/// The two sources a stop condition may name.
///
/// The model types `StopConditionSource` as a plain string, but FIS documents the value:
/// "Specify `aws:cloudwatch:alarm` if the stop condition is defined by a CloudWatch
/// alarm. Specify `none` if there is no stop condition." Anything else is refused as a
/// `ValidationException`.
pub const STOP_CONDITION_SOURCE_NONE: &str = "none";

/// See [`STOP_CONDITION_SOURCE_NONE`]. A stop condition naming this source carries the
/// alarm's ARN in `value`, which FIS documents as required for it.
pub const STOP_CONDITION_SOURCE_ALARM: &str = "aws:cloudwatch:alarm";

/// How long after the call that starts it a transition this service owes falls due.
///
/// AWS publishes no duration for `initiating` -> `running` or `stopping` -> `stopped`, so
/// this span is the emulator's own choice. It is deliberately NOT zero: a caller that
/// reads the experiment straight after `StartExperiment` or `StopExperiment` observes the
/// transient status, exactly as it would against the real service.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The `state.reason` an experiment carries once `StopExperiment` has been called on it.
///
/// FIS documents no wording for `ExperimentState$reason` ("The reason for the state"), so
/// this is an explanatory string rather than a reproduction of the service's own text.
pub const STOPPED_BY_REQUEST: &str = "Stopped by a StopExperiment request";

/// The status an experiment this emulator persists can be in.
///
/// `ExperimentStatus` is modelled
/// `pending | initiating | running | completed | stopping | stopped | failed | cancelled`.
/// Only the four below are reachable here: an experiment is minted `initiating`, becomes
/// `running` when the clock says so, and goes `stopping` -> `stopped` when
/// `StopExperiment` is called. `completed`, `failed` and `cancelled` report the outcome
/// of RUNNING an experiment's actions — injecting faults into other services — which this
/// emulator does not perform, so it never claims one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentStatus {
    /// The experiment has been accepted and is starting up.
    Initiating,
    /// The experiment is live.
    Running,
    /// A stop has been requested and is being carried out.
    Stopping,
    /// The experiment is over. Terminal.
    Stopped,
}

impl ExperimentStatus {
    /// The wire value of this status — one of `ExperimentStatus`'s enum values.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Initiating => "initiating",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }

    /// True when no further transition can leave this status.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped)
    }
}

/// One target account configuration of an experiment template, exactly as
/// `CreateTargetAccountConfiguration` stored it.
///
/// A target account configuration is a SUB-RESOURCE of an experiment template, named by
/// the (`experimentTemplateId`, `accountId`) pair rather than by an id the service mints
/// — so it carries no id and no ARN, and it lives on the template record it belongs to.
#[derive(Debug, Clone)]
pub struct TargetAccountConfigurationRecord {
    /// `TargetAccountId` — the account this configuration targets, and the second half
    /// of its name.
    pub account_id: String,
    /// `RoleArn` — "the Amazon Resource Name (ARN) of an IAM role for the target
    /// account", stored exactly as the request carried it.
    pub role_arn: String,
    /// `TargetAccountConfigurationDescription`, absent when the request named none.
    ///
    /// Held as an `Option` so a read reports what the caller actually sent: an absent
    /// description stays absent and an empty one stays an empty string, rather than the
    /// two collapsing into one value.
    pub description: Option<String>,
    /// The `@idempotencyToken` of the request that created this configuration, when it
    /// carried one.
    ///
    /// This sub-resource is named by its (template, account) pair, so a token cannot be
    /// resolved through [`ComAmazonawsFisFaultinjectionsimulatorState::idempotency`],
    /// which maps a token to a single minted id; the token is kept on the record and
    /// compared by [`TargetAccountConfigurationRecord::is_replay_of`] instead.
    pub client_token: Option<String>,
}

impl TargetAccountConfigurationRecord {
    /// The record `CreateTargetAccountConfiguration` persists for one account.
    pub fn new(
        account_id: String,
        role_arn: String,
        description: Option<String>,
        client_token: Option<String>,
    ) -> Self {
        Self {
            account_id,
            role_arn,
            description,
            client_token,
        }
    }

    /// True when a create request carrying `client_token` is a RETRY of the request that
    /// created this record, rather than a second create for the same account.
    ///
    /// A request that carries no token is never a replay: there is nothing to match it
    /// against, so a second create for the same account is a conflict.
    pub fn is_replay_of(&self, client_token: Option<&str>) -> bool {
        match (self.client_token.as_deref(), client_token) {
            (Some(stored), Some(offered)) => stored == offered,
            _ => false,
        }
    }

    /// Apply an `UpdateTargetAccountConfiguration` request to this record.
    ///
    /// `roleArn` and `description` are both OPTIONAL on that request while `roleArn` is
    /// required on the create, so the update is a partial one: a member the request
    /// carries replaces the stored value, and a member it omits leaves the stored value
    /// exactly as it was. An empty `description` IS carried
    /// (`TargetAccountConfigurationDescription` is modelled `@length(min: 0)`), so it
    /// replaces the stored description with the empty string rather than counting as
    /// "omitted".
    ///
    /// The `accountId` names the record and so is never changed by an update, and the
    /// creating `@idempotencyToken` is kept: a retry of the create still resolves to this
    /// record, and reports it in whatever state it is now in.
    pub fn apply_update(&mut self, role_arn: Option<&str>, description: Option<&str>) {
        if let Some(role_arn) = role_arn {
            self.role_arn = role_arn.to_string();
        }
        if let Some(description) = description {
            self.description = Some(description.to_string());
        }
    }

    /// The `TargetAccountConfiguration` every target-account-configuration operation
    /// returns: every member is the stored one.
    pub fn to_target_account_configuration(&self) -> TargetAccountConfiguration {
        TargetAccountConfiguration {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }

    /// The `TargetAccountConfigurationSummary` a page of
    /// `ListTargetAccountConfigurations` carries.
    ///
    /// `TargetAccountConfigurationSummary` declares the same three members as
    /// `TargetAccountConfiguration`, so a listed configuration reports exactly what a
    /// `GetTargetAccountConfiguration` of the same name reports — including an absent
    /// `description` staying absent.
    pub fn to_summary(&self) -> TargetAccountConfigurationSummary {
        TargetAccountConfigurationSummary {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }

    /// The `ExperimentTargetAccountConfigurationSummary` a page of
    /// `ListExperimentTargetAccountConfigurations` carries.
    ///
    /// `ExperimentTargetAccountConfigurationSummary` declares the same three members as
    /// `ExperimentTargetAccountConfiguration`, so a listed configuration of an experiment
    /// reports exactly what a `GetExperimentTargetAccountConfiguration` of the same name
    /// reports — including an absent `description` staying absent.
    pub fn to_experiment_summary(&self) -> ExperimentTargetAccountConfigurationSummary {
        ExperimentTargetAccountConfigurationSummary {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }

    /// The same record as an `ExperimentTargetAccountConfiguration` — the shape an
    /// EXPERIMENT's target account configuration is reported as.
    ///
    /// `ExperimentTargetAccountConfiguration` and `TargetAccountConfiguration` carry the
    /// same three members (`roleArn`, `accountId`, `description`), so an experiment
    /// reports the configuration its template held when it was started, member for
    /// member.
    pub fn to_experiment_target_account_configuration(
        &self,
    ) -> ExperimentTargetAccountConfiguration {
        ExperimentTargetAccountConfiguration {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }
}

/// One experiment template, exactly as `CreateExperimentTemplate` stored it.
#[derive(Debug, Clone)]
pub struct ExperimentTemplateRecord {
    /// `ExperimentTemplateId`, minted by [`ComAmazonawsFisFaultinjectionsimulatorState::mint_template_id`].
    pub id: String,
    /// `ResourceArn`, built by [`template_arn`] for the region the request was signed for.
    pub arn: String,
    pub description: String,
    pub targets: HashMap<String, ExperimentTemplateTarget>,
    pub actions: HashMap<String, ExperimentTemplateAction>,
    pub stop_conditions: Vec<ExperimentTemplateStopCondition>,
    pub role_arn: String,
    pub tags: HashMap<String, String>,
    pub log_configuration: Option<ExperimentTemplateLogConfiguration>,
    pub experiment_options: ExperimentTemplateExperimentOptions,
    pub experiment_report_configuration: Option<ExperimentTemplateReportConfiguration>,
    pub creation_time: SystemTime,
    /// Set to `creation_time` when the record is created, and moved only by a call that
    /// changes the template.
    pub last_update_time: SystemTime,
    /// Every target account configuration of this template, keyed by its `accountId`.
    /// `CreateTargetAccountConfiguration` adds one and
    /// `targetAccountConfigurationsCount` is derived from this map, so the count a read
    /// reports is the number of configurations the store actually holds. A `BTreeMap` is
    /// what gives `ListTargetAccountConfigurations` a stable total order.
    pub target_account_configurations: BTreeMap<String, TargetAccountConfigurationRecord>,
}

impl ExperimentTemplateRecord {
    /// The record `CreateExperimentTemplate` persists for `input`.
    pub fn from_create_input(
        id: String,
        arn: String,
        input: &CreateExperimentTemplateInput,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            arn,
            description: input.description.clone(),
            targets: input
                .targets
                .iter()
                .flatten()
                .map(|(name, target)| (name.clone(), template_target(target)))
                .collect(),
            actions: input
                .actions
                .iter()
                .map(|(name, action)| (name.clone(), template_action(action)))
                .collect(),
            stop_conditions: input
                .stop_conditions
                .iter()
                .map(template_stop_condition)
                .collect(),
            role_arn: input.role_arn.clone(),
            tags: input.tags.clone().unwrap_or_default(),
            log_configuration: input.log_configuration.as_ref().map(template_log_configuration),
            experiment_options: template_experiment_options(input.experiment_options.as_ref()),
            experiment_report_configuration: input
                .experiment_report_configuration
                .as_ref()
                .map(template_report_configuration),
            creation_time: now,
            last_update_time: now,
            target_account_configurations: BTreeMap::new(),
        }
    }

    /// `targetAccountConfigurationsCount` — how many target account configurations this
    /// template holds, derived from the stored configurations rather than counted
    /// separately.
    pub fn target_account_configurations_count(&self) -> i64 {
        i64::try_from(self.target_account_configurations.len()).unwrap_or(i64::MAX)
    }

    /// The `ExperimentTemplate` every template operation returns: every member is the
    /// stored one.
    pub fn to_experiment_template(&self) -> ExperimentTemplate {
        ExperimentTemplate {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            description: Some(self.description.clone()),
            targets: Some(self.targets.clone()),
            actions: Some(self.actions.clone()),
            stop_conditions: Some(self.stop_conditions.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_update_time: Some(::aws_smithy_types::DateTime::from(self.last_update_time)),
            role_arn: Some(self.role_arn.clone()),
            tags: Some(self.tags.clone()),
            log_configuration: self.log_configuration.clone(),
            experiment_options: Some(self.experiment_options.clone()),
            target_account_configurations_count: Some(self.target_account_configurations_count()),
            experiment_report_configuration: self.experiment_report_configuration.clone(),
        }
    }

    /// The `ExperimentTemplateSummary` `ListExperimentTemplates` returns for this record.
    pub fn to_summary(&self) -> ExperimentTemplateSummary {
        ExperimentTemplateSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            description: Some(self.description.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_update_time: Some(::aws_smithy_types::DateTime::from(self.last_update_time)),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One experiment, exactly as `StartExperiment` stored it and `StopExperiment` left it.
#[derive(Debug, Clone)]
pub struct ExperimentRecord {
    /// `ExperimentId`, minted by [`ComAmazonawsFisFaultinjectionsimulatorState::mint_experiment_id`].
    pub id: String,
    /// `ResourceArn`, built by [`experiment_arn`].
    pub arn: String,
    /// The template this experiment was started from.
    pub experiment_template_id: String,
    /// Copied from that template.
    pub role_arn: String,
    pub status: ExperimentStatus,
    pub reason: Option<String>,
    /// Copied from the template, as `ExperimentTarget`s.
    pub targets: HashMap<String, ExperimentTarget>,
    /// What those targets RESOLVED to when this experiment was started, in the order
    /// `ListExperimentResolvedTargets` reports them.
    ///
    /// A SNAPSHOT, computed once by [`resolve_targets`] at start time: FIS resolves an
    /// experiment's targets when the experiment starts, so the resolution a read reports
    /// is the one the start computed, however often it is read and whatever happens to
    /// the template afterwards.
    pub resolved_targets: Vec<ResolvedTarget>,
    /// Copied from the template, as `ExperimentAction`s.
    pub actions: HashMap<String, ExperimentAction>,
    /// Copied from the template, as `ExperimentStopCondition`s.
    pub stop_conditions: Vec<ExperimentStopCondition>,
    pub tags: HashMap<String, String>,
    /// The template's options, plus the `actionsMode` `StartExperiment` chose.
    pub experiment_options: ExperimentOptions,
    pub log_configuration: Option<ExperimentLogConfiguration>,
    pub experiment_report_configuration: Option<ExperimentReportConfiguration>,
    /// The target account configurations the template carried when this experiment was
    /// started, keyed by `accountId`.
    ///
    /// A SNAPSHOT: an experiment runs against the accounts the template named at start
    /// time, so a configuration added to or removed from the template afterwards does
    /// not change this experiment. `targetAccountConfigurationsCount` is derived from it.
    pub target_account_configurations: BTreeMap<String, TargetAccountConfigurationRecord>,
    pub creation_time: SystemTime,
    pub start_time: SystemTime,
    /// "The time that the experiment ended" — set when the experiment reaches a terminal
    /// status, and absent until then.
    pub end_time: Option<SystemTime>,
    /// When the transition this experiment owes the clock falls due, or `None` when it
    /// owes none. [`ExperimentRecord::settle`] is what spends it.
    pub transition_due: Option<SystemTime>,
}

impl ExperimentRecord {
    /// The record `StartExperiment` persists for `template`.
    pub fn from_template(
        id: String,
        arn: String,
        template: &ExperimentTemplateRecord,
        actions_mode: &str,
        tags: Option<HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        let targets: HashMap<String, ExperimentTarget> = template
            .targets
            .iter()
            .map(|(name, target)| (name.clone(), experiment_target(target)))
            .collect();
        // Resolution happens ONCE, here, where the experiment is started.
        let resolved_targets = resolve_targets(&targets);
        Self {
            id,
            arn,
            experiment_template_id: template.id.clone(),
            role_arn: template.role_arn.clone(),
            // An accepted experiment starts up before it runs; the clock owes the move to
            // `running`.
            status: ExperimentStatus::Initiating,
            reason: None,
            targets,
            resolved_targets,
            actions: template
                .actions
                .iter()
                .map(|(name, action)| (name.clone(), experiment_action(action)))
                .collect(),
            stop_conditions: template
                .stop_conditions
                .iter()
                .map(experiment_stop_condition)
                .collect(),
            tags: tags.unwrap_or_default(),
            experiment_options: ExperimentOptions {
                account_targeting: template.experiment_options.account_targeting.clone(),
                empty_target_resolution_mode: template
                    .experiment_options
                    .empty_target_resolution_mode
                    .clone(),
                actions_mode: Some(actions_mode.to_string()),
            },
            log_configuration: template
                .log_configuration
                .as_ref()
                .map(experiment_log_configuration),
            experiment_report_configuration: template
                .experiment_report_configuration
                .as_ref()
                .map(experiment_report_configuration),
            target_account_configurations: template.target_account_configurations.clone(),
            creation_time: now,
            start_time: now,
            end_time: None,
            transition_due: Some(now + TRANSITION_DELAY),
        }
    }

    /// `targetAccountConfigurationsCount` — how many target account configurations this
    /// experiment was started with, derived from the snapshot it carries.
    pub fn target_account_configurations_count(&self) -> i64 {
        i64::try_from(self.target_account_configurations.len()).unwrap_or(i64::MAX)
    }

    /// Materialise the transition the clock already owes at `now`, if it is due.
    ///
    /// This is the one write a REPORTING operation performs, and it only makes a change
    /// that was owed at `now` whether or not anybody called: an experiment that was
    /// `initiating` when [`TRANSITION_DELAY`] elapsed is `running` from that instant, and
    /// one that was `stopping` is `stopped` from it. Nothing here depends on the call
    /// being made, so two reads at the same instant report the same status.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.transition_due else {
            return;
        };
        if now < due {
            return;
        }
        self.transition_due = None;
        match self.status {
            ExperimentStatus::Initiating => self.status = ExperimentStatus::Running,
            ExperimentStatus::Stopping => {
                self.status = ExperimentStatus::Stopped;
                // The experiment ended when the transition fell due, not when somebody
                // happened to look.
                self.end_time = Some(due);
            }
            ExperimentStatus::Running | ExperimentStatus::Stopped => {}
        }
    }

    /// Start the move to `stopped`, due [`TRANSITION_DELAY`] from `now`.
    ///
    /// A no-op on an experiment that is already stopping or stopped, so a repeated
    /// `StopExperiment` neither restarts the span nor moves `endTime`.
    pub fn request_stop(&mut self, now: SystemTime) {
        if matches!(
            self.status,
            ExperimentStatus::Stopping | ExperimentStatus::Stopped
        ) {
            return;
        }
        self.status = ExperimentStatus::Stopping;
        self.reason = Some(STOPPED_BY_REQUEST.to_string());
        self.transition_due = Some(now + TRANSITION_DELAY);
    }

    /// The `ExperimentState` this record reports.
    pub fn to_state(&self) -> ExperimentState {
        ExperimentState {
            status: Some(self.status.as_wire().to_string()),
            reason: self.reason.clone(),
            // `ExperimentError` carries "the error information of the experiment when the
            // action has failed"; no status this emulator reaches is a failure.
            error: None,
        }
    }

    /// The `Experiment` every experiment operation returns: every member is the stored
    /// one.
    pub fn to_experiment(&self) -> Experiment {
        Experiment {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            experiment_template_id: Some(self.experiment_template_id.clone()),
            role_arn: Some(self.role_arn.clone()),
            state: Some(self.to_state()),
            targets: Some(self.targets.clone()),
            actions: Some(self.actions.clone()),
            stop_conditions: Some(self.stop_conditions.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            tags: Some(self.tags.clone()),
            log_configuration: self.log_configuration.clone(),
            experiment_options: Some(self.experiment_options.clone()),
            target_account_configurations_count: Some(self.target_account_configurations_count()),
            experiment_report_configuration: self.experiment_report_configuration.clone(),
            // `ExperimentReport` describes a report this emulator does not generate: the
            // report configuration is persisted, but no report is produced for it.
            experiment_report: None,
        }
    }

    /// The `ExperimentSummary` `ListExperiments` returns for this record.
    pub fn to_summary(&self) -> ExperimentSummary {
        ExperimentSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            experiment_template_id: Some(self.experiment_template_id.clone()),
            state: Some(self.to_state()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            tags: Some(self.tags.clone()),
            experiment_options: Some(self.experiment_options.clone()),
        }
    }
}

/// Which resource an idempotency token created, so a token a client reuses across two
/// different operations is not mistaken for a replay of the other one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdempotentResource {
    ExperimentTemplate,
    Experiment,
}

/// Per-service state for com-amazonaws-fis-faultinjectionsimulator. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsFisFaultinjectionsimulatorState {
    /// Every experiment template, keyed by its id. A `BTreeMap` is what gives
    /// `ListExperimentTemplates` the stable total order its pagination token indexes
    /// into; ids are minted fixed-width, so that order is creation order.
    pub experiment_templates: BTreeMap<String, ExperimentTemplateRecord>,
    /// Every experiment, keyed by its id — ordered for the same reason.
    pub experiments: BTreeMap<String, ExperimentRecord>,
    /// The resource each `@idempotencyToken` has already created, so replaying a token
    /// returns the first resource instead of minting a second one.
    ///
    /// An entry is dropped when the resource it names leaves the store, which is what
    /// keeps this map bounded by the live records.
    pub idempotency: BTreeMap<(IdempotentResource, String), String>,
    /// The counter template ids are minted from, so two calls never share an id.
    pub template_seq: u64,
    /// The counter experiment ids are minted from.
    pub experiment_seq: u64,
}

impl ComAmazonawsFisFaultinjectionsimulatorState {
    /// A fresh experiment-template id, which no earlier call has used.
    pub fn mint_template_id(&mut self) -> String {
        self.template_seq += 1;
        format_id(TEMPLATE_ID_PREFIX, self.template_seq)
    }

    /// A fresh experiment id, which no earlier call has used.
    pub fn mint_experiment_id(&mut self) -> String {
        self.experiment_seq += 1;
        format_id(EXPERIMENT_ID_PREFIX, self.experiment_seq)
    }

    /// The resource `token` already created for `resource`, when it is still in the store.
    pub fn replayed(&self, resource: IdempotentResource, token: &str) -> Option<&String> {
        let id = self.idempotency.get(&(resource, token.to_string()))?;
        let live = match resource {
            IdempotentResource::ExperimentTemplate => self.experiment_templates.contains_key(id),
            IdempotentResource::Experiment => self.experiments.contains_key(id),
        };
        live.then_some(id)
    }

    /// Record that `token` created `id`.
    pub fn remember_token(&mut self, resource: IdempotentResource, token: &str, id: &str) {
        self.idempotency
            .insert((resource, token.to_string()), id.to_string());
    }

    /// Drop every idempotency entry that names `id`, so the map does not outlive the
    /// records it points at.
    pub fn forget_tokens_for(&mut self, resource: IdempotentResource, id: &str) {
        self.idempotency
            .retain(|(kind, _), created| *kind != resource || created != id);
    }

    /// The experiment `id` names, with every transition the clock already owes at `now`
    /// materialised. `None` when the store holds no such experiment.
    pub fn settled_experiment_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut ExperimentRecord> {
        let record = self.experiments.get_mut(id)?;
        record.settle(now);
        Some(record)
    }

    /// Every experiment, with each one's owed transition materialised at `now`. This is
    /// what `ListExperiments` reports from, so a listed status agrees with the status
    /// `GetExperiment` reports for the same experiment at the same instant.
    pub fn settle_experiments(&mut self, now: SystemTime) {
        for record in self.experiments.values_mut() {
            record.settle(now);
        }
    }
}

/// `prefix` followed by [`ID_SUFFIX_LEN`] base-36 characters holding `seq`.
///
/// Fixed width and upper-case base 36 mean the lexicographic order of two minted ids is
/// their minting order, which is the order a paginated list walks and resumes in.
fn format_id(prefix: &str, seq: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    // The invariant the pagination tokens rest on: a sequence number that needs more
    // than ID_SUFFIX_LEN digits would wrap, and the lexicographic order of two ids would
    // stop being their minting order. Unreachable while the store is bounded by
    // MAX_EXPERIMENT_TEMPLATES / MAX_EXPERIMENTS, so this states the invariant rather
    // than correcting anything.
    debug_assert!(
        seq < 36u64.pow(ID_SUFFIX_LEN as u32),
        "sequence number {seq} does not fit in {ID_SUFFIX_LEN} base-36 digits"
    );
    let mut suffix = [b'0'; ID_SUFFIX_LEN];
    let mut rest = seq;
    for slot in suffix.iter_mut().rev() {
        *slot = DIGITS[usize::try_from(rest % 36).unwrap_or(0)];
        rest /= 36;
    }
    // Every byte came from DIGITS, so the suffix is ASCII.
    format!("{prefix}{}", String::from_utf8_lossy(&suffix))
}

/// True when `value` has the shape [`format_id`] mints with `prefix`.
///
/// The boundary validator refuses a modelled id member that is empty or carries
/// whitespace, so this is for a value the model does not constrain beyond that — a
/// `nextToken`, which a paginated list carries a resource id in.
pub fn is_minted_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    suffix.len() == ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

/// The `nextToken` a page of `ListTargetAccountConfigurations` hands back when
/// configurations remain: the marker of [`TARGET_ACCOUNT_PAGE_TOKEN_PREFIX`] followed by
/// the `accountId` the next page resumes at.
///
/// `NextToken` is modelled `@length(min: 1, max: 1024)`, `pattern ^[\S]+$`: the marker
/// carries no whitespace, an `accountId` that passed validation carries none either, and
/// the two together are far under the ceiling.
pub fn target_account_page_token(account_id: &str) -> String {
    format!("{TARGET_ACCOUNT_PAGE_TOKEN_PREFIX}{account_id}")
}

/// The `accountId` the page token `token` resumes at, or `None` when `token` is not one
/// [`target_account_page_token`] could have issued — the marker followed by a value within
/// `TargetAccountId`'s modelled bounds.
pub fn target_account_page_token_account(token: &str) -> Option<&str> {
    let account_id = token.strip_prefix(TARGET_ACCOUNT_PAGE_TOKEN_PREFIX)?;
    let length = account_id.chars().count();
    ((TARGET_ACCOUNT_ID_FLOOR..=TARGET_ACCOUNT_ID_CEILING).contains(&length)
        && !account_id.chars().any(char::is_whitespace))
    .then_some(account_id)
}

/// The `nextToken` a page of `ListExperimentResolvedTargets` hands back when resolved
/// targets remain: the marker of [`RESOLVED_TARGET_PAGE_TOKEN_PREFIX`] followed by the
/// POSITION in the experiment's resolution the next page resumes at.
///
/// `NextToken` is modelled `@length(min: 1, max: 1024)`, `pattern ^[\S]+$`: the marker
/// carries no whitespace, a decimal position carries none either, and the two together
/// are far under the ceiling.
pub fn resolved_target_page_token(index: usize) -> String {
    format!("{RESOLVED_TARGET_PAGE_TOKEN_PREFIX}{index}")
}

/// The position the page token `token` resumes at, or `None` when `token` is not one
/// [`resolved_target_page_token`] could have issued — the marker followed by a decimal
/// position.
pub fn resolved_target_page_token_index(token: &str) -> Option<usize> {
    let index = token.strip_prefix(RESOLVED_TARGET_PAGE_TOKEN_PREFIX)?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    index.parse().ok()
}

/// What `targets` RESOLVE to: one `ResolvedTarget` per resource a target identifies,
/// scoped by that target's `selectionMode`.
///
/// The resources a target identifies are the ones it NAMES (`resourceArns`), each named
/// resource resolving once however often the target lists it. A target that identifies
/// its resources by `resourceTags` instead describes resources of OTHER services, which
/// this emulator does not hold and therefore cannot resolve — it identifies nothing here,
/// and nothing is reported for it rather than a resource being invented for it.
///
/// The order is a stable total order — targets by name, and within one target the order
/// it named its resources — which is what [`resolved_target_page_token`] indexes into.
///
/// The work is linear and bounded: de-duplication goes through a set, one target names at
/// most [`MAX_TARGET_RESOURCE_ARNS`] resources (the model's own ceiling, refused at the
/// boundary), and the resolution stops at [`MAX_RESOLVED_TARGETS`].
pub fn resolve_targets(targets: &HashMap<String, ExperimentTarget>) -> Vec<ResolvedTarget> {
    let mut by_name: Vec<(&String, &ExperimentTarget)> = targets.iter().collect();
    by_name.sort_by_key(|(name, _)| *name);
    let mut resolved = Vec::new();
    for (name, target) in by_name {
        // [`MAX_RESOLVED_TARGETS`] is the emulator's own ceiling on one experiment's
        // resolution: the work this function does, and the records it hands to the store,
        // are bounded by it rather than by how long a list a request carried.
        if resolved.len() >= MAX_RESOLVED_TARGETS {
            break;
        }
        // One resource per ARN: an ARN a target repeats names the resource it already
        // identified. Membership goes through a SET, so de-duplicating a target's list
        // costs time linear in its length rather than quadratic.
        let mut seen: HashSet<&str> = HashSet::new();
        let mut identified: Vec<&String> = Vec::new();
        for arn in target.resource_arns.iter().flatten() {
            if seen.insert(arn.as_str()) {
                identified.push(arn);
            }
        }
        let keep = selection_count(target.selection_mode.as_deref(), identified.len())
            .min(MAX_RESOLVED_TARGETS - resolved.len());
        for arn in identified.into_iter().take(keep) {
            resolved.push(ResolvedTarget {
                resource_type: target.resource_type.clone(),
                target_name: Some(name.clone()),
                target_information: Some(HashMap::from([(
                    TARGET_INFORMATION_ARN_KEY.to_string(),
                    arn.clone(),
                )])),
            });
        }
    }
    resolved
}

/// How many of the `identified` resources a target's `selectionMode` keeps.
///
/// FIS documents three forms: `ALL` ("Run the action on all identified targets. This is
/// the default"), `COUNT(n)` ("the specified number of targets") and `PERCENT(n)` ("the
/// specified percentage of targets"). A count or a percentage asking for more than the
/// target identified keeps what it identified.
///
/// Two choices this emulator makes, where FIS documents no rule: a percentage that does
/// not reach a whole resource is taken DOWN (`PERCENT(25)` of two resources keeps none,
/// of four keeps one), and a value in none of the three documented forms is read as the
/// documented default, `ALL`, rather than scoping anything away. FIS picks the kept
/// resources "at random"; this emulator keeps the first ones in the order the target
/// named them, which satisfies the documented count and makes a resolution reproducible.
pub fn selection_count(selection_mode: Option<&str>, identified: usize) -> usize {
    let Some(mode) = selection_mode.map(str::trim) else {
        return identified;
    };
    if mode == SELECTION_MODE_ALL {
        return identified;
    }
    if let Some(count) = selection_argument(mode, SELECTION_MODE_COUNT_PREFIX) {
        return identified.min(count);
    }
    if let Some(percent) = selection_argument(mode, SELECTION_MODE_PERCENT_PREFIX) {
        return identified.saturating_mul(percent.min(100)) / 100;
    }
    // `ALL`, and anything the create accepted that is in none of the documented forms:
    // the default scopes nothing away. No panic and no refusal here — `selectionMode` is
    // modelled as a plain string, so a value outside the documented forms reaches the
    // store, and reporting every identified resource for it is the documented default.
    identified
}

/// The `n` of a `selectionMode` of the form `<prefix>n)`, or `None` when `mode` is not of
/// that form. The documented forms are upper-case, so the match is case-sensitive.
fn selection_argument(mode: &str, prefix: &str) -> Option<usize> {
    mode.strip_prefix(prefix)?.strip_suffix(')')?.trim().parse().ok()
}

/// The ARN of the experiment template `id` names, in `region`.
pub fn template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{TEMPLATE_RESOURCE}/{id}"))
}

/// The ARN of the experiment `id` names, in `region`.
pub fn experiment_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{EXPERIMENT_RESOURCE}/{id}"))
}

/// The ARN of the FIS action `id` names, in `region`.
///
/// Formatted here rather than through `::engine::emulator_arn` for one reason: that
/// helper always interpolates the emulator account, and an action's ARN has an EMPTY
/// account segment because the action belongs to the service rather than to an account
/// (see [`ACTION_RESOURCE`]). Every account-owned resource this service mints still goes
/// through the helper — [`template_arn`] and [`experiment_arn`].
pub fn action_arn(region: &str, id: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{region}::{ACTION_RESOURCE}/{id}")
}

/// The region this request was signed for — what a minted ARN names.
///
/// The scope's region is only honoured when it has an AWS region's SHAPE
/// (`<partition>-<area>[-<area>]-<number>`, e.g. `us-east-1`, `ap-southeast-3`): the
/// value is interpolated into an ARN, so an arbitrary header value must not be able to
/// shape one. Anything else falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `value` has an AWS region's shape: lowercase alphabetic segments joined by
/// `-`, ending in a digit group, at least three segments.
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The page size a paginated FIS list serves for `max_results`.
///
/// Every `maxResults` member is modelled `@range(min: 1, max: 100)` and the boundary
/// validator refused anything outside it before a handler ran, so the only case left is
/// an absent value: serve the largest page the model allows. The `clamp` states that
/// invariant rather than correcting a caller — a value reaching here outside the range is
/// a validator bug, which the debug assertion surfaces instead of silently repairing.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(PAGE_LIMIT);
    debug_assert!(
        (1..=PAGE_LIMIT).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).unwrap_or(1)
}

/// `None` when every stop condition in `conditions` satisfies what FIS documents for
/// `StopCondition`; otherwise the `ValidationException` that refuses the request.
///
/// Two rules the model cannot express: `source` is one of
/// [`STOP_CONDITION_SOURCE_NONE`] / [`STOP_CONDITION_SOURCE_ALARM`], and a stop condition
/// naming the alarm source carries the alarm's ARN in `value` ("This is required if the
/// source is a CloudWatch alarm").
pub fn stop_conditions_refusal(
    conditions: &[CreateExperimentTemplateStopConditionInput],
) -> Option<ValidationException> {
    for (index, condition) in conditions.iter().enumerate() {
        let source = condition.source.as_str();
        if source != STOP_CONDITION_SOURCE_NONE && source != STOP_CONDITION_SOURCE_ALARM {
            return Some(invalid_input(format!(
                "stopConditions[{index}].source must be \
                 {STOP_CONDITION_SOURCE_NONE} or {STOP_CONDITION_SOURCE_ALARM}, got {source:?}"
            )));
        }
        if source == STOP_CONDITION_SOURCE_ALARM
            && condition
                .value
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Some(invalid_input(format!(
                "stopConditions[{index}].value is required when source is \
                 {STOP_CONDITION_SOURCE_ALARM}"
            )));
        }
    }
    None
}

/// `None` when `value` is absent or is one of `admitted`; otherwise the
/// `ValidationException` that refuses the member `member` carries.
///
/// The boundary validator does not check enum membership, so an operation that persists
/// an enum-typed member checks it here rather than storing a value the model does not
/// admit.
pub fn enum_refusal(
    member: &str,
    value: Option<&str>,
    admitted: &[&str],
) -> Option<ValidationException> {
    let value = value?;
    if admitted.contains(&value) {
        return None;
    }
    Some(invalid_input(format!(
        "{member} must be one of {}, got {value:?}",
        admitted.join(" | ")
    )))
}

/// The `ValidationException` refusals of the experiment options a
/// `CreateExperimentTemplate` request carries.
pub fn template_experiment_options_refusal(
    options: Option<&CreateExperimentTemplateExperimentOptionsInput>,
) -> Option<ValidationException> {
    let options = options?;
    enum_refusal(
        "experimentOptions.accountTargeting",
        options.account_targeting.as_deref(),
        ACCOUNT_TARGETING_VALUES,
    )
    .or_else(|| {
        enum_refusal(
            "experimentOptions.emptyTargetResolutionMode",
            options.empty_target_resolution_mode.as_deref(),
            EMPTY_TARGET_RESOLUTION_MODE_VALUES,
        )
    })
}

/// The `actionsMode` an experiment runs under, or the `ValidationException` that refuses
/// the value the request named.
pub fn resolve_actions_mode(requested: Option<&str>) -> Result<String, ValidationException> {
    if let Some(refusal) = enum_refusal(
        "experimentOptions.actionsMode",
        requested,
        ACTIONS_MODE_VALUES,
    ) {
        return Err(refusal);
    }
    Ok(requested.unwrap_or(DEFAULT_ACTIONS_MODE).to_string())
}

/// `None` when `role_arn` is absent or is a well-formed IAM role ARN; otherwise the
/// `ValidationException` that refuses the member `member` carries.
///
/// `RoleArn` is modelled only as a string within `@length(20..2048)`, `pattern ^[\S]+$`,
/// so the ARN SYNTAX of a role reference is a server-side rule: FIS refuses a value that
/// satisfies those bounds but is not an IAM role ARN with a 400 `ValidationException`,
/// before anything is persisted. Every member of this service that names an IAM role
/// (`CreateExperimentTemplate.roleArn`, `CreateTargetAccountConfiguration.roleArn`,
/// `UpdateTargetAccountConfiguration.roleArn`) goes through here.
pub fn role_arn_refusal(member: &str, role_arn: Option<&str>) -> Option<ValidationException> {
    let role_arn = role_arn?;
    if is_iam_role_arn(role_arn) {
        return None;
    }
    Some(invalid_input(format!(
        "Value for {member} is not a valid ARN."
    )))
}

/// True when `value` is a well-formed IAM role ARN:
/// `arn:<partition>:iam::<12-digit account id>:role/<path segments>/<role name>`.
///
/// IAM is a global service, so the region segment of one of its ARNs is EMPTY and the
/// account segment is an AWS account number (twelve digits). The resource is a role
/// (`role/` followed by an optional path and the role name), and no segment of that path
/// may be empty. The characters of the role name itself are NOT constrained here beyond
/// what the modelled `pattern ^[\S]+$` already refuses: the rule this checks is the ARN's
/// STRUCTURE.
pub fn is_iam_role_arn(value: &str) -> bool {
    // `splitn(6, ':')` keeps any further colon inside the resource segment, which
    // [`is_iam_role_resource`] then refuses: an IAM role resource carries none.
    let segments: Vec<&str> = value.splitn(6, ':').collect();
    let ["arn", partition, "iam", "", account_id, resource] = segments.as_slice() else {
        return false;
    };
    is_partition(partition) && is_account_id(account_id) && is_iam_role_resource(resource)
}

/// True when `value` has an AWS partition's shape (`aws`, `aws-cn`, `aws-us-gov`, …):
/// non-empty lowercase alphanumeric segments joined by `-`.
fn is_partition(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// True when `value` is an AWS account number: exactly twelve decimal digits.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|b| b.is_ascii_digit())
}

/// True when `value` is the resource segment of an IAM ROLE ARN: `role/`, then an
/// optional path and the role name, with no empty segment and no colon.
fn is_iam_role_resource(value: &str) -> bool {
    let Some(path_and_name) = value.strip_prefix("role/") else {
        return false;
    };
    // An empty value splits into one EMPTY segment, which the segment rule below already
    // refuses: `role/` on its own names no role. The characters of a segment are not
    // constrained here — `RoleArn`'s modelled `pattern ^[\S]+$` already refused
    // whitespace at the boundary — so what this checks is the resource's STRUCTURE.
    !path_and_name.contains(':') && path_and_name.split('/').all(|segment| !segment.is_empty())
}

/// The `ValidationException` payload for a request this service refuses.
pub fn invalid_input(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(message.into()),
    }
}

/// The `ValidationException` payload for a `nextToken` this service never issued.
///
/// FIS answers one constant for every paginated list of this service — the text does not
/// vary with the token, the operation or the resource it paginates — so the message is
/// built here rather than by each list handler.
pub fn invalid_next_token() -> ValidationException {
    invalid_input(INVALID_NEXT_TOKEN)
}

/// The `ResourceNotFoundException` payload for an experiment template id that names no
/// record — "the specified resource cannot be found".
///
/// FIS answers the CONSTANT [`EXPERIMENT_TEMPLATE_NOT_FOUND`] here, so this takes NO
/// argument: the message neither names nor echoes the identifier the request asked for,
/// whether that id was never minted, is malformed-but-well-typed, or named a template
/// that has since been deleted. Every operation of this service that resolves an
/// `experimentTemplateId` (`GetExperimentTemplate`, `DeleteExperimentTemplate`,
/// `StartExperiment`, `CreateTargetAccountConfiguration`,
/// `ListTargetAccountConfigurations`) refuses the miss through here, so they cannot
/// disagree.
pub fn template_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(EXPERIMENT_TEMPLATE_NOT_FOUND.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for an action id the catalog does not hold —
/// "the specified resource cannot be found".
pub fn action_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("No action found with id {id}")),
    }
}

/// The `ResourceNotFoundException` payload for an experiment id that names no record.
pub fn experiment_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("No experiment found with id {id}")),
    }
}

/// The `ResourceNotFoundException` payload for a (template, account) pair that names no
/// target account configuration — "the specified resource cannot be found".
///
/// FIS answers the CONSTANT [`TARGET_ACCOUNT_CONFIGURATION_NOT_FOUND`] here: the text
/// names neither the template nor the account, so it takes no argument.
pub fn target_account_configuration_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TARGET_ACCOUNT_CONFIGURATION_NOT_FOUND.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for an (experiment, account) pair that names
/// no target account configuration on that experiment's snapshot — "the specified
/// resource cannot be found".
///
/// Unlike the template-scoped miss ([`target_account_configuration_not_found`], a
/// constant), FIS builds this message FROM THE REQUEST: it names the experiment first and
/// then the target account.
pub fn experiment_target_account_configuration_not_found(
    experiment_id: &str,
    account_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "TargetAccountConfiguration for experiment {experiment_id} \
             and target account {account_id} not found"
        )),
    }
}

/// The `ConflictException` payload for a create that names an account the template
/// already has a target account configuration for — "the request could not be processed
/// because of a conflict".
pub fn target_account_configuration_exists(
    experiment_template_id: &str,
    account_id: &str,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Experiment template {experiment_template_id} already has a target account \
             configuration for account {account_id}"
        )),
    }
}

/// The `ServiceQuotaExceededException` payload for a create that would push the store
/// past the bound it holds.
pub fn store_full(resource: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "this emulator already holds the maximum of {limit} {resource} records"
        )),
    }
}

/// The experiment options a template carries: what the request named, with FIS's
/// documented default filling in each member it left out.
fn template_experiment_options(
    input: Option<&CreateExperimentTemplateExperimentOptionsInput>,
) -> ExperimentTemplateExperimentOptions {
    ExperimentTemplateExperimentOptions {
        account_targeting: Some(
            input
                .and_then(|options| options.account_targeting.clone())
                .unwrap_or_else(|| DEFAULT_ACCOUNT_TARGETING.to_string()),
        ),
        empty_target_resolution_mode: Some(
            input
                .and_then(|options| options.empty_target_resolution_mode.clone())
                .unwrap_or_else(|| DEFAULT_EMPTY_TARGET_RESOLUTION_MODE.to_string()),
        ),
    }
}

fn template_target(input: &CreateExperimentTemplateTargetInput) -> ExperimentTemplateTarget {
    ExperimentTemplateTarget {
        resource_type: Some(input.resource_type.clone()),
        resource_arns: input.resource_arns.clone(),
        resource_tags: input.resource_tags.clone(),
        filters: input
            .filters
            .as_ref()
            .map(|filters| filters.iter().map(template_target_filter).collect()),
        selection_mode: Some(input.selection_mode.clone()),
        parameters: input.parameters.clone(),
    }
}

fn template_target_filter(
    input: &ExperimentTemplateTargetInputFilter,
) -> ExperimentTemplateTargetFilter {
    ExperimentTemplateTargetFilter {
        path: Some(input.path.clone()),
        values: Some(input.values.clone()),
    }
}

fn template_action(input: &CreateExperimentTemplateActionInput) -> ExperimentTemplateAction {
    ExperimentTemplateAction {
        action_id: Some(input.action_id.clone()),
        description: input.description.clone(),
        parameters: input.parameters.clone(),
        targets: input.targets.clone(),
        start_after: input.start_after.clone(),
    }
}

fn template_stop_condition(
    input: &CreateExperimentTemplateStopConditionInput,
) -> ExperimentTemplateStopCondition {
    ExperimentTemplateStopCondition {
        source: Some(input.source.clone()),
        value: input.value.clone(),
    }
}

fn template_log_configuration(
    input: &CreateExperimentTemplateLogConfigurationInput,
) -> ExperimentTemplateLogConfiguration {
    ExperimentTemplateLogConfiguration {
        cloud_watch_logs_configuration: input
            .cloud_watch_logs_configuration
            .as_ref()
            .map(template_cloud_watch_logs_configuration),
        s3_configuration: input.s3_configuration.as_ref().map(template_s3_configuration),
        log_schema_version: Some(input.log_schema_version),
    }
}

fn template_cloud_watch_logs_configuration(
    input: &ExperimentTemplateCloudWatchLogsLogConfigurationInput,
) -> ExperimentTemplateCloudWatchLogsLogConfiguration {
    ExperimentTemplateCloudWatchLogsLogConfiguration {
        log_group_arn: Some(input.log_group_arn.clone()),
    }
}

fn template_s3_configuration(
    input: &ExperimentTemplateS3LogConfigurationInput,
) -> ExperimentTemplateS3LogConfiguration {
    ExperimentTemplateS3LogConfiguration {
        bucket_name: Some(input.bucket_name.clone()),
        prefix: input.prefix.clone(),
    }
}

fn template_report_configuration(
    input: &CreateExperimentTemplateReportConfigurationInput,
) -> ExperimentTemplateReportConfiguration {
    ExperimentTemplateReportConfiguration {
        outputs: input.outputs.as_ref().map(template_report_outputs),
        data_sources: input.data_sources.as_ref().map(template_report_data_sources),
        pre_experiment_duration: input.pre_experiment_duration.clone(),
        post_experiment_duration: input.post_experiment_duration.clone(),
    }
}

fn template_report_outputs(
    input: &ExperimentTemplateReportConfigurationOutputsInput,
) -> ExperimentTemplateReportConfigurationOutputs {
    ExperimentTemplateReportConfigurationOutputs {
        s3_configuration: input.s3_configuration.as_ref().map(
            |s3: &ReportConfigurationS3OutputInput| ReportConfigurationS3Output {
                bucket_name: s3.bucket_name.clone(),
                prefix: s3.prefix.clone(),
            },
        ),
    }
}

fn template_report_data_sources(
    input: &ExperimentTemplateReportConfigurationDataSourcesInput,
) -> ExperimentTemplateReportConfigurationDataSources {
    ExperimentTemplateReportConfigurationDataSources {
        cloud_watch_dashboards: input.cloud_watch_dashboards.as_ref().map(|dashboards| {
            dashboards
                .iter()
                .map(|dashboard: &ReportConfigurationCloudWatchDashboardInput| {
                    ExperimentTemplateReportConfigurationCloudWatchDashboard {
                        dashboard_identifier: dashboard.dashboard_identifier.clone(),
                    }
                })
                .collect()
        }),
    }
}

fn experiment_target(target: &ExperimentTemplateTarget) -> ExperimentTarget {
    ExperimentTarget {
        resource_type: target.resource_type.clone(),
        resource_arns: target.resource_arns.clone(),
        resource_tags: target.resource_tags.clone(),
        filters: target.filters.as_ref().map(|filters| {
            filters
                .iter()
                .map(|filter| ExperimentTargetFilter {
                    path: filter.path.clone(),
                    values: filter.values.clone(),
                })
                .collect()
        }),
        selection_mode: target.selection_mode.clone(),
        parameters: target.parameters.clone(),
    }
}

/// The experiment's copy of one of the template's actions.
///
/// `state`, `startTime` and `endTime` are left unset on purpose: they report the progress
/// of an action that injects a fault into another service, and this emulator runs no
/// action, so it reports no per-action status rather than claiming one for work nothing
/// performed.
fn experiment_action(action: &ExperimentTemplateAction) -> ExperimentAction {
    ExperimentAction {
        action_id: action.action_id.clone(),
        description: action.description.clone(),
        parameters: action.parameters.clone(),
        targets: action.targets.clone(),
        start_after: action.start_after.clone(),
        state: None,
        start_time: None,
        end_time: None,
    }
}

fn experiment_stop_condition(
    condition: &ExperimentTemplateStopCondition,
) -> ExperimentStopCondition {
    ExperimentStopCondition {
        source: condition.source.clone(),
        value: condition.value.clone(),
    }
}

fn experiment_log_configuration(
    configuration: &ExperimentTemplateLogConfiguration,
) -> ExperimentLogConfiguration {
    ExperimentLogConfiguration {
        cloud_watch_logs_configuration: configuration
            .cloud_watch_logs_configuration
            .as_ref()
            .map(|logs| ExperimentCloudWatchLogsLogConfiguration {
                log_group_arn: logs.log_group_arn.clone(),
            }),
        s3_configuration: configuration
            .s3_configuration
            .as_ref()
            .map(|s3| ExperimentS3LogConfiguration {
                bucket_name: s3.bucket_name.clone(),
                prefix: s3.prefix.clone(),
            }),
        log_schema_version: configuration.log_schema_version,
    }
}

fn experiment_report_configuration(
    configuration: &ExperimentTemplateReportConfiguration,
) -> ExperimentReportConfiguration {
    ExperimentReportConfiguration {
        outputs: configuration
            .outputs
            .as_ref()
            .map(|outputs| ExperimentReportConfigurationOutputs {
                s3_configuration: outputs.s3_configuration.as_ref().map(|s3| {
                    ExperimentReportConfigurationOutputsS3Configuration {
                        bucket_name: s3.bucket_name.clone(),
                        prefix: s3.prefix.clone(),
                    }
                }),
            }),
        data_sources: configuration.data_sources.as_ref().map(|sources| {
            ExperimentReportConfigurationDataSources {
                cloud_watch_dashboards: sources.cloud_watch_dashboards.as_ref().map(
                    |dashboards| {
                        dashboards
                            .iter()
                            .map(|dashboard| ExperimentReportConfigurationCloudWatchDashboard {
                                dashboard_identifier: dashboard.dashboard_identifier.clone(),
                            })
                            .collect()
                    },
                ),
            }
        }),
        pre_experiment_duration: configuration.pre_experiment_duration.clone(),
        post_experiment_duration: configuration.post_experiment_duration.clone(),
    }
}

/// One parameter of a built-in FIS action, as [`ACTION_CATALOG`] records it.
#[derive(Debug, Clone, Copy)]
pub struct ActionCatalogParameter {
    /// The key the parameter is named by in `Action$parameters`.
    pub name: &'static str,
    /// `ActionParameter$description`.
    pub description: &'static str,
    /// `ActionParameter$required` — whether an experiment template's action must set it.
    pub required: bool,
}

/// One supported target of a built-in FIS action, as [`ACTION_CATALOG`] records it.
#[derive(Debug, Clone, Copy)]
pub struct ActionCatalogTarget {
    /// The key the target is named by in `Action$targets`, which is also the key an
    /// experiment template's action refers to a target by.
    pub name: &'static str,
    /// `ActionTarget$resourceType` — a `TargetResourceTypeId`.
    pub resource_type: &'static str,
}

/// One built-in FIS action.
///
/// An action is SERVICE-owned: FIS publishes the catalog and no API creates, updates or
/// deletes an entry, so `GetAction` and `ListActions` report from this table rather than
/// from anything a caller stored. The table is what makes those two operations agree.
#[derive(Debug, Clone, Copy)]
pub struct ActionCatalogEntry {
    /// `ActionId` — the fully qualified action id (`aws:<service>:<operation>`).
    pub id: &'static str,
    /// `ActionDescription`.
    pub description: &'static str,
    /// The action's parameters, empty for an action that takes none.
    pub parameters: &'static [ActionCatalogParameter],
    /// The target resource types the action supports, empty for an action that needs no
    /// target.
    pub targets: &'static [ActionCatalogTarget],
}

impl ActionCatalogEntry {
    /// The `Action` `GetAction` reports for this entry, in `region`.
    ///
    /// `tags` is an empty map: an action is service-owned, so there is nothing for a
    /// caller to have tagged on it.
    pub fn to_action(&self, region: &str) -> Action {
        Action {
            id: Some(self.id.to_string()),
            arn: Some(action_arn(region, self.id)),
            description: Some(self.description.to_string()),
            parameters: Some(
                self.parameters
                    .iter()
                    .map(|parameter| {
                        (
                            parameter.name.to_string(),
                            ActionParameter {
                                description: Some(parameter.description.to_string()),
                                required: Some(parameter.required),
                            },
                        )
                    })
                    .collect(),
            ),
            targets: Some(
                self.targets
                    .iter()
                    .map(|target| {
                        (
                            target.name.to_string(),
                            ActionTarget {
                                resource_type: Some(target.resource_type.to_string()),
                            },
                        )
                    })
                    .collect(),
            ),
            tags: Some(HashMap::new()),
        }
    }
}

/// The built-in FIS actions this emulator serves.
///
/// SOURCE: the AWS FIS actions reference
/// (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
/// `Action`'s own model documentation points at ("Describes an action. For more
/// information, see FIS actions in the Fault Injection Service User Guide"). Each entry
/// records what that reference states for the action: its id, a one-line summary of what
/// it does, the parameters it accepts and which of them are required, and the target
/// resource types it supports under the key an experiment template refers to them by.
///
/// It is a SUBSET of the published catalog, not the whole of it: an id this table does
/// not hold answers `ResourceNotFoundException` rather than a fabricated action.
///
/// Kept sorted by id, which is the order `ListActions` enumerates in.
pub const ACTION_CATALOG: &[ActionCatalogEntry] = &[
    ActionCatalogEntry {
        id: "aws:cloudwatch:assert-alarm-state",
        description: "Verify that the specified alarms are in one of the specified alarm states.",
        parameters: &[
            ActionCatalogParameter {
                name: "alarmArns",
                description: "The Amazon Resource Names (ARNs) of the alarms, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "alarmStates",
                description: "The alarm states, separated by commas.",
                required: true,
            },
        ],
        targets: &[],
    },
    ActionCatalogEntry {
        id: "aws:ebs:pause-volume-io",
        description: "Pause I/O operations on the target EBS volumes.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "Volumes",
            resource_type: "aws:ec2:ebs-volume",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ec2:api-insufficient-instance-capacity-error",
        description: "Cause EC2 RunInstances requests made with the target IAM roles to fail with \
                      an insufficient instance capacity error.",
        parameters: &[
            ActionCatalogParameter {
                name: "availabilityZoneIdentifiers",
                description: "The Availability Zones, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ec2:asg-insufficient-instance-capacity-error",
        description: "Cause instance launches in the target Auto Scaling groups to fail with an \
                      insufficient instance capacity error.",
        parameters: &[
            ActionCatalogParameter {
                name: "availabilityZoneIdentifiers",
                description: "The Availability Zones, separated by commas.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "AutoScalingGroups",
            resource_type: "aws:ec2:autoscaling-group",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ec2:reboot-instances",
        description: "Reboot the target EC2 instances.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ec2:send-spot-instance-interruptions",
        description: "Interrupt the target Spot Instances.",
        parameters: &[ActionCatalogParameter {
            name: "durationBeforeInterruption",
            description: "The length of time before the interruption notice is sent, in ISO 8601 \
                          format.",
            required: false,
        }],
        targets: &[ActionCatalogTarget {
            name: "SpotInstances",
            resource_type: "aws:ec2:spot-instance",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ec2:stop-instances",
        description: "Stop the target EC2 instances.",
        parameters: &[
            ActionCatalogParameter {
                name: "completeIfInstancesTerminated",
                description: "Whether to complete the action if the target instances were \
                              terminated instead of stopped.",
                required: false,
            },
            ActionCatalogParameter {
                name: "startInstancesAfterDuration",
                description: "The length of time to wait before starting the instances again, in \
                              ISO 8601 format.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ec2:terminate-instances",
        description: "Terminate the target EC2 instances.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ecs:drain-container-instances",
        description: "Drain the container instances of the target Amazon ECS clusters.",
        parameters: &[
            ActionCatalogParameter {
                name: "drainagePercentage",
                description: "The percentage of container instances to drain.",
                required: true,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Clusters",
            resource_type: "aws:ecs:cluster",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ecs:stop-task",
        description: "Stop the target Amazon ECS tasks.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
    },
    ActionCatalogEntry {
        id: "aws:eks:terminate-nodegroup-instances",
        description: "Terminate instances in the target Amazon EKS node groups.",
        parameters: &[ActionCatalogParameter {
            name: "instanceTerminationPercentage",
            description: "The percentage of instances to terminate.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "Nodegroups",
            resource_type: "aws:eks:nodegroup",
        }],
    },
    ActionCatalogEntry {
        id: "aws:fis:inject-api-internal-error",
        description: "Cause API requests made with the target IAM roles to fail with an internal \
                      error.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The AWS service whose API operations are affected.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
    },
    ActionCatalogEntry {
        id: "aws:fis:inject-api-throttle-error",
        description: "Cause API requests made with the target IAM roles to be throttled.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The AWS service whose API operations are affected.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
    },
    ActionCatalogEntry {
        id: "aws:fis:inject-api-unavailable-error",
        description: "Cause API requests made with the target IAM roles to fail with an \
                      unavailable error.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The AWS service whose API operations are affected.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
    },
    ActionCatalogEntry {
        id: "aws:fis:wait",
        description: "Wait for the specified duration.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[],
    },
    ActionCatalogEntry {
        id: "aws:network:disrupt-connectivity",
        description: "Disrupt network connectivity for the target subnets.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "scope",
                description: "The traffic to disrupt.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Subnets",
            resource_type: "aws:ec2:subnet",
        }],
    },
    ActionCatalogEntry {
        id: "aws:rds:failover-db-cluster",
        description: "Fail over the target Amazon Aurora DB clusters.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Clusters",
            resource_type: "aws:rds:cluster",
        }],
    },
    ActionCatalogEntry {
        id: "aws:rds:reboot-db-instances",
        description: "Reboot the target Amazon RDS DB instances.",
        parameters: &[ActionCatalogParameter {
            name: "forceFailover",
            description: "Whether to force a failover of a Multi-AZ DB instance.",
            required: false,
        }],
        targets: &[ActionCatalogTarget {
            name: "DBInstances",
            resource_type: "aws:rds:db",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ssm:send-command",
        description: "Run the specified SSM document on the target instances.",
        parameters: &[
            ActionCatalogParameter {
                name: "documentArn",
                description: "The Amazon Resource Name (ARN) of the document.",
                required: true,
            },
            ActionCatalogParameter {
                name: "documentParameters",
                description: "The document parameters, as a JSON object.",
                required: false,
            },
            ActionCatalogParameter {
                name: "documentVersion",
                description: "The version of the document.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
    },
    ActionCatalogEntry {
        id: "aws:ssm:start-automation-execution",
        description: "Run the specified SSM automation document.",
        parameters: &[
            ActionCatalogParameter {
                name: "documentArn",
                description: "The Amazon Resource Name (ARN) of the automation document.",
                required: true,
            },
            ActionCatalogParameter {
                name: "documentParameters",
                description: "The document parameters, as a JSON object.",
                required: false,
            },
            ActionCatalogParameter {
                name: "documentVersion",
                description: "The version of the document.",
                required: false,
            },
            ActionCatalogParameter {
                name: "maxConcurrency",
                description: "The maximum number of automation executions to run at the same \
                              time.",
                required: false,
            },
            ActionCatalogParameter {
                name: "maxErrors",
                description: "The maximum number of allowed automation execution failures.",
                required: false,
            },
        ],
        targets: &[],
    },
];

/// The catalog entry `id` names, or `None` when FIS publishes no such action.
///
/// `ActionId` is matched exactly: an action id is case-sensitive, so a differently cased
/// or abbreviated id names no action.
pub fn action_catalog_entry(id: &str) -> Option<&'static ActionCatalogEntry> {
    ACTION_CATALOG.iter().find(|entry| entry.id == id)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFisFaultinjectionsimulatorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFisFaultinjectionsimulatorState>)
}
