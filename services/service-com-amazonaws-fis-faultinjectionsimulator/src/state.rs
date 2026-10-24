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
//! transition (`initiating` -> `running`, `running` -> `completed`/`failed`, `stopping`
//! -> `stopped`): the instant it falls due is stored on the record and materialised by
//! [`ExperimentRecord::settle`], so no handler ever sleeps and a read never invents a
//! status the clock did not already owe. An experiment that is STARTED therefore runs:
//! it is observable in `running` for as long as the longest path through the actions it
//! CARRIES OUT takes on the emulator's run clock ([`run_plan`], [`RUN_CLOCK_SCALE`]) and
//! then reaches the terminal status those actions earned — `failed` with an
//! insufficient-privileges reason when the account's IAM store shows the experiment's
//! execution role may not perform an action's own API calls
//! ([`action_permissions`]), and `completed` otherwise.
//!
//! WHICH actions a run carries out is what the experiment options decide ([`RunActions`],
//! [`plan_run`]): `actionsMode: skip-all` carries out none of them, and
//! `emptyTargetResolutionMode` says what an empty target resolution does — `skip` skips
//! the actions that name such a target, `fail` (the default) ends the run `failed` on the
//! resolution. An action the run does not carry out spends none of its `duration`, earns
//! no failure attributed to it, and reports the modelled `skipped` status.
//!
//! Two things that run means here, and does NOT:
//!
//! * `completed` is the status of a RUN that elapsed, not a claim that an action's effect
//!   was produced. What this service models is the experiment's own lifecycle, which is
//!   FIS's record of the run; the effect of an action on another service (an
//!   `aws:fis:inject-api-*` fault making the targeted role's calls to another service fail,
//!   an instance stopping) happens in THAT service's responses, and this emulator does not
//!   produce it. Nothing here reports such an effect as having happened: no injected fault
//!   is announced, and the per-action `state`/`startTime`/`endTime` an experiment could
//!   carry are left unset rather than filled in for work nothing performed — the one
//!   exception being the action the run SKIPS, whose `state` is the decision the options
//!   made and not a claim about work (see `experiment_action` and [`ActionSkip`]).
//! * the time members `startTime` and `endTime` bound the run on the COMPRESSED clock
//!   ([`RUN_CLOCK_SCALE`]), so the span between them is shorter than the duration the
//!   actions name — which is reported back verbatim, exactly as the template stored it.
//!   That is deliberate, and the constant says why: an uncompressed run would make every
//!   terminal status unobservable for up to the documented twelve hours.
//!
//! A template may also carry TARGET ACCOUNT CONFIGURATIONS, one per account id, which
//! `UpdateTargetAccountConfiguration` changes in place. A configuration names one account
//! and the role FIS assumes in it, so the two have to agree: the role has to belong to
//! that account ([`target_account_role_arn_refusal`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Action, ActionParameter, ActionTarget, ConflictException, CreateExperimentTemplateActionInput,
    CreateExperimentTemplateExperimentOptionsInput, CreateExperimentTemplateInput,
    CreateExperimentTemplateLogConfigurationInput,
    CreateExperimentTemplateReportConfigurationInput, CreateExperimentTemplateStopConditionInput,
    CreateExperimentTemplateTargetInput, Experiment, ExperimentAction, ExperimentActionState,
    ExperimentCloudWatchLogsLogConfiguration, ExperimentLogConfiguration, ExperimentOptions,
    ExperimentReportConfiguration, ExperimentReportConfigurationCloudWatchDashboard,
    ExperimentReportConfigurationDataSources, ExperimentReportConfigurationOutputs,
    ExperimentReportConfigurationOutputsS3Configuration, ExperimentS3LogConfiguration,
    ExperimentError, ExperimentState, ExperimentStopCondition, ExperimentSummary,
    ExperimentTarget,
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
    ResourceNotFoundException, ServiceQuotaExceededException, StartExperimentInput,
    TargetAccountConfiguration, TargetAccountConfigurationSummary, ValidationException,
    ACCOUNT_TARGETING_VALUES, ACTIONS_MODE_VALUES, EMPTY_TARGET_RESOLUTION_MODE_VALUES,
};

pub mod action_permissions;

/// The IAM store this service resolves an execution role against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so FIS reads the very records `CreateRole` wrote rather
/// than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

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

/// How many experiments may be ACTIVE at once before `StartExperiment` refuses with
/// `ServiceQuotaExceededException`.
///
/// Unlike [`MAX_EXPERIMENTS`] this is a quota AWS publishes: the FIS user guide's quotas
/// page lists "Active experiments" as 5 per account per Region, non-adjustable. The store
/// is not partitioned by region, so the count is taken over every experiment it holds.
pub const MAX_ACTIVE_EXPERIMENTS: usize = 5;

/// How many target account configurations ONE experiment template holds before
/// `CreateTargetAccountConfiguration` refuses with `ServiceQuotaExceededException`.
///
/// Like [`MAX_EXPERIMENT_TEMPLATES`] this is the emulator's OWN bound on its store
/// rather than a claim about a quota AWS publishes, and it is checked only when a
/// request would ADD a configuration.
pub const MAX_TARGET_ACCOUNT_CONFIGURATIONS: usize = 1_000;

/// How many actions ONE experiment template may carry.
///
/// A quota AWS publishes: the FIS user guide's quotas page lists "Actions per experiment
/// template" as 20. It is also what BOUNDS the work [`run_plan`] does for a request — the
/// action graph is walked once per action, so a stated cap on the actions keeps that walk
/// linear in the size of the request instead of growing with it.
pub const MAX_ACTIONS_PER_TEMPLATE: usize = 20;

/// The built-in FIS action that waits: "aws:fis:wait — Run a wait action for a specified
/// duration". Its whole effect is elapsed time, and it is the action whose `duration`
/// parameter FIS documents a range for.
pub const WAIT_ACTION_ID: &str = "aws:fis:wait";

/// The parameter an action's own length comes from: "duration — The duration of the
/// action", in ISO 8601 format, which the FIS actions reference gives that meaning for
/// every built-in action that takes it (required on `aws:fis:wait`, where it is "the
/// duration of the wait").
pub const DURATION_PARAMETER: &str = "duration";

/// The smallest wait `aws:fis:wait` accepts: the documented range of its `duration` is
/// 1 to 720 minutes (12 hours).
pub const WAIT_DURATION_MIN_MINUTES: u64 = 1;

/// The largest wait `aws:fis:wait` accepts. See [`WAIT_DURATION_MIN_MINUTES`].
pub const WAIT_DURATION_MAX_MINUTES: u64 = 720;

/// The service principal a template's execution role must let assume it, so FIS can run
/// the experiment under that role.
pub const FIS_SERVICE_PRINCIPAL: &str = "fis.amazonaws.com";

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

/// The `emptyTargetResolutionMode` that SKIPS the action instead of failing the
/// experiment: FIS documents the two values as "fail — the experiment fails if a target
/// resolves to zero resources" (the default,
/// [`DEFAULT_EMPTY_TARGET_RESOLUTION_MODE`]) and "skip — the experiment skips the action
/// if a target resolves to zero resources". [`run_actions`] is where they are read.
pub const EMPTY_TARGET_RESOLUTION_MODE_SKIP: &str = "skip";

/// The value FIS applies to `actionsMode` when `StartExperiment` names none: the
/// experiment runs all of its actions (`ActionsMode` is modelled `run-all | skip-all`).
pub const DEFAULT_ACTIONS_MODE: &str = "run-all";

/// The `actionsMode` that carries out NONE of the experiment's actions.
///
/// FIS documents `skip-all` as skipping all of the actions, which is how a caller checks
/// an experiment's TARGET RESOLUTION without performing the actions: the experiment is
/// started and resolves its targets, and nothing is carried out. See [`run_actions`].
pub const ACTIONS_MODE_SKIP_ALL: &str = "skip-all";

/// `ExperimentActionStatus`'s value for an action the run did not carry out \u2014 one of the
/// modelled `ExperimentActionStatus` values
/// (`pending | initiating | running | completed | cancelled | stopping | stopped | failed |
/// skipped`), and the only one this emulator reports: it states the DECISION the
/// experiment options made about the action rather than any progress of work.
pub const EXPERIMENT_ACTION_STATUS_SKIPPED: &str = "skipped";

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

/// How far the MODELLED clock of an experiment's run is compressed against the wall
/// clock: one modelled minute per second of real time.
///
/// What this scales is how long the EMULATOR takes to walk a run, not what the run says it
/// is: an action's `duration` is reported back verbatim, exactly as the template stored
/// it. FIS documents `aws:fis:wait`'s `duration` as 1 to 720 minutes, so an uncompressed
/// run would hold every caller that waits for an experiment to finish — and every test of
/// one — for between a minute and twelve hours before any terminal status became readable.
/// The run is therefore driven on this compressed clock, clamped into
/// [`MIN_RUN_SPAN`]..=[`MAX_RUN_SPAN`], which keeps the compression monotonic (a longer
/// modelled run is never a shorter real one) while every transient status stays observable
/// and a terminal one arrives promptly.
pub const RUN_CLOCK_SCALE: u32 = 60;

/// The shortest a run takes once the experiment is `running`, whatever its actions'
/// durations compress to — an experiment whose actions name no duration at all included.
///
/// Never zero, and deliberately several poll intervals long: `running` is a status FIS
/// reports for every experiment it starts, so a client that polls a started experiment has
/// to be able to SEE it. See [`RUN_CLOCK_SCALE`] for why a run is compressed at all.
pub const MIN_RUN_SPAN: Duration = Duration::from_secs(5);

/// The longest a run takes, whatever duration its actions name. See [`RUN_CLOCK_SCALE`].
pub const MAX_RUN_SPAN: Duration = Duration::from_secs(15);

/// `ExperimentStatusReason`'s modelled ceiling (`@length(min: 0, max: 512)`).
///
/// A state reason this service builds from the request — the role ARN of an experiment
/// that failed on privileges, say, which `RoleArn` bounds at 2,048 characters — is clipped
/// to it, so a value the caller chose cannot push a response outside the member's own
/// bounds.
pub const EXPERIMENT_STATUS_REASON_CEILING: usize = 512;

/// The `state.reason` an experiment carries once `StopExperiment` has been called on it.
///
/// FIS documents no wording for `ExperimentState$reason` ("The reason for the state"), so
/// this is an explanatory string rather than a reproduction of the service's own text.
pub const STOPPED_BY_REQUEST: &str = "Stopped by a StopExperiment request";

/// The `state.reason` an experiment carries once its actions have run to the end. Chosen
/// here for the same reason [`STOPPED_BY_REQUEST`] is.
pub const COMPLETED_REASON: &str = "Experiment completed";

/// The status an experiment this emulator persists can be in.
///
/// `ExperimentStatus` is modelled
/// `pending | initiating | running | completed | stopping | stopped | failed | cancelled`.
/// Six of the eight are reachable here: an experiment is minted `initiating`, becomes
/// `running` when the clock says so, and then either `completed` (its actions ran to the
/// end of the run) or `failed` (an action could not be run under the experiment's
/// execution role); a `StopExperiment` moves it `stopping` -> `stopped` instead. `pending`
/// is the status FIS reports before it has accepted the run, and `cancelled` reports a run
/// cancelled by something other than a stop request, neither of which this emulator
/// produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentStatus {
    /// The experiment has been accepted and is starting up.
    Initiating,
    /// The experiment is live.
    Running,
    /// A stop has been requested and is being carried out.
    Stopping,
    /// The experiment was stopped. Terminal.
    Stopped,
    /// The experiment's actions ran to the end. Terminal.
    Completed,
    /// The experiment's actions could not be carried out. Terminal.
    Failed,
}

impl ExperimentStatus {
    /// The wire value of this status — one of `ExperimentStatus`'s enum values.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Initiating => "initiating",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// True when no further transition can leave this status.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Completed | Self::Failed)
    }

    /// True while the experiment still occupies one of the account's active-experiment
    /// slots — every status that is not terminal. This is the count
    /// [`MAX_ACTIVE_EXPERIMENTS`] is taken against.
    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// What the end of an experiment's run is: the terminal status its actions earn once the
/// run span elapses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentOutcome {
    /// Every action of the experiment ran to the end of the run, so it ends `completed`.
    Completed,
    /// An action could not be run at all, so the run ends in the modelled `failed` state
    /// carrying this failure.
    Failed(ExperimentFailure),
}

/// Why an experiment's run ended `failed`: the `ExperimentState$reason` it reports and the
/// `ExperimentError` that goes with it.
///
/// FIS reports BOTH for a failed experiment — "the reason for the state" and "the error
/// information of the experiment when the action has failed" — and that pair is what says
/// WHICH failure it was, so a client can tell a run AWS would have refused on IAM from one
/// that failed for any other reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentFailure {
    /// `ExperimentState$reason`, within [`EXPERIMENT_STATUS_REASON_CEILING`].
    pub reason: String,
    /// `ExperimentError$code` — "the error code for the failed experiment", modelled only
    /// as a string within `@length(0..128)`, `pattern ^[\S]+$`.
    pub code: String,
    /// `ExperimentError$location` — "context for the section of the experiment template
    /// that failed", which is the action that could not be run: `actions.<name>`, the same
    /// path this service's input refusals name an action by.
    pub location: String,
}

impl ExperimentFailure {
    /// The failure of an action the experiment's execution role is not allowed to perform.
    ///
    /// Every value in the reason comes from the request and the account's own stored
    /// policies — the role the template names, the action that could not run, and the IAM
    /// action that role is not allowed — around the attribution AWS words an
    /// insufficient-privileges failure with, which is what makes this failure
    /// distinguishable from every other one.
    pub fn insufficient_privileges(
        role_arn: &str,
        action_name: &str,
        action_id: &str,
        permission: &str,
    ) -> Self {
        Self {
            reason: clipped_reason(format!(
                "Not enough privileges to run action {action_name} ({action_id}): the \
                 experiment role {role_arn} is not allowed to perform {permission}"
            )),
            code: INSUFFICIENT_PRIVILEGES_CODE.to_string(),
            location: format!("actions.{action_name}"),
        }
    }

    /// The failure of a run whose TARGET RESOLUTION came back empty, under the
    /// `emptyTargetResolutionMode` that fails the experiment for it
    /// ([`DEFAULT_EMPTY_TARGET_RESOLUTION_MODE`], which FIS documents as "the experiment
    /// fails if a target resolves to zero resources").
    ///
    /// The target's own name comes from the template, and the location is the section of
    /// the template that failed — the TARGET rather than an action, because the resolution
    /// is what came back empty and no action had run when it did.
    pub fn empty_target_resolution(target_name: &str) -> Self {
        Self {
            reason: clipped_reason(format!(
                "Target {target_name} resolved to no resources and \
                 experimentOptions.emptyTargetResolutionMode is \
                 {DEFAULT_EMPTY_TARGET_RESOLUTION_MODE}"
            )),
            code: EMPTY_TARGET_RESOLUTION_CODE.to_string(),
            location: format!("targets.{target_name}"),
        }
    }

    /// The `ExperimentError` a failed experiment reports beside its reason.
    ///
    /// `accountId` is "the Amazon Web Services Account ID where the experiment failure
    /// occurred" — this emulator's account, which is the account the experiment ran in.
    pub fn to_error(&self) -> ExperimentError {
        ExperimentError {
            account_id: Some(::engine::emulator_account_id().to_string()),
            code: Some(self.code.clone()),
            location: Some(self.location.clone()),
        }
    }
}

/// The `ExperimentError$code` of a run that failed because the experiment's execution role
/// is not allowed to perform an action's own API calls.
///
/// FIS publishes no catalog of `ExperimentErrorCode` values, so this is the emulator's own
/// token for that condition rather than a reproduction of AWS's: what the contract
/// requires is a code within the member's modelled `@length(0..128)` / `^[\S]+$` that
/// tells this failure apart from any other, which is the same thing the reason does in
/// prose.
pub const INSUFFICIENT_PRIVILEGES_CODE: &str = "insufficientPrivileges";

/// The `ExperimentError$code` of a run that ended because one of the experiment's targets
/// resolved to no resources while its `emptyTargetResolutionMode` was the one that fails
/// for it.
///
/// Like [`INSUFFICIENT_PRIVILEGES_CODE`] this is the emulator's own token rather than a
/// reproduction of AWS's, which publishes no catalog of `ExperimentErrorCode` values: what
/// the contract requires is a code within the member's modelled `@length(0..128)` /
/// `^[\S]+$` that tells this failure apart from any other.
pub const EMPTY_TARGET_RESOLUTION_CODE: &str = "emptyTargetResolution";

/// `reason` within `ExperimentStatusReason`'s modelled ceiling, cut at a character
/// boundary so a clipped reason is still valid UTF-8.
///
/// `ExperimentActionStatusReason` ("the reason for the state" of one ACTION) is modelled
/// with the same `@length(min: 0, max: 512)`, so the reason of a skipped action
/// ([`ActionSkip::reason`]) is clipped here too.
fn clipped_reason(reason: String) -> String {
    if reason.chars().count() <= EXPERIMENT_STATUS_REASON_CEILING {
        return reason;
    }
    reason
        .chars()
        .take(EXPERIMENT_STATUS_REASON_CEILING)
        .collect()
}

/// Why a started experiment does NOT carry out one of the template's actions.
///
/// Both values are an experiment OPTION's doing, which is the only thing that skips an
/// action here: the emulator never decides on its own that an action was not performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSkip {
    /// The experiment was started with `actionsMode: skip-all`, which carries out no
    /// action at all ([`ACTIONS_MODE_SKIP_ALL`]).
    AllActions,
    /// The target of this name, which the action refers to, resolved to no resources while
    /// the experiment's `emptyTargetResolutionMode` was
    /// [`EMPTY_TARGET_RESOLUTION_MODE_SKIP`].
    EmptyTarget(String),
}

impl ActionSkip {
    /// The `ExperimentActionState` a skipped action reports: the modelled `skipped` status
    /// and, in `reason`, the option that skipped it.
    ///
    /// FIS documents no WORDING for `ExperimentActionState$reason` ("the reason for the
    /// state"), so the text is an explanatory string of this emulator's own — like
    /// [`STOPPED_BY_REQUEST`] — while the status is the value the model names for an action
    /// that was skipped.
    pub fn to_state(&self) -> ExperimentActionState {
        ExperimentActionState {
            status: Some(EXPERIMENT_ACTION_STATUS_SKIPPED.to_string()),
            reason: Some(clipped_reason(match self {
                Self::AllActions => format!(
                    "Action skipped: the experiment was started with \
                     experimentOptions.actionsMode {ACTIONS_MODE_SKIP_ALL}"
                ),
                Self::EmptyTarget(target) => format!(
                    "Action skipped: target {target} resolved to no resources and \
                     experimentOptions.emptyTargetResolutionMode is \
                     {EMPTY_TARGET_RESOLUTION_MODE_SKIP}"
                ),
            })),
        }
    }
}

/// What the experiment options make of a template's actions when an experiment is started
/// from it: which actions the run CARRIES OUT, which it skips and why, and the failure the
/// target resolution earned before any action ran.
///
/// This is the one place the two options that select the run are read, and every
/// consequence of them follows from it: only an action the run carries out spends its
/// `duration` ([`run_plan`]) or is decided against the execution role's permissions
/// ([`action_permissions::permission_failure`]), because an action nothing carries out can
/// neither take time nor earn a failure attributed to it.
#[derive(Debug, Clone, Default)]
pub struct RunActions {
    /// The actions the run carries out, by the name the template gave them.
    pub carried_out: HashMap<String, ExperimentTemplateAction>,
    /// The actions the run does not carry out, and why — the `state` each of them reports.
    ///
    /// Ordered by name, so the skipped actions of one start are reported in one order
    /// rather than in whichever one a `HashMap` yielded.
    pub skipped: BTreeMap<String, ActionSkip>,
    /// The failure an empty target resolution earns under the `emptyTargetResolutionMode`
    /// that fails for it, or `None` when the resolution gave that mode nothing to fail on.
    pub empty_target_failure: Option<ExperimentFailure>,
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
    /// How long this experiment is `running` for — the longest path through the actions it
    /// CARRIES OUT on the emulator's run clock, as [`run_plan`] computes it over the plan
    /// [`RunActions`] made of the template.
    pub run_span: Duration,
    /// The terminal status the run earns once [`run_span`](Self::run_span) elapses.
    pub outcome: ExperimentOutcome,
}

impl ExperimentRecord {
    /// The record `StartExperiment` persists for `template`.
    ///
    /// `actions_mode` is the `actionsMode` the start chose, which with the template's
    /// `emptyTargetResolutionMode` decides WHICH of the template's actions this run carries
    /// out ([`run_actions`]).
    ///
    /// `failure` is what the start already knows about the run: the one failure the
    /// account's IAM store SHOWS the experiment's execution role cannot get past (see
    /// [`action_permissions::permission_failure`]), or `None` when it shows none. It is
    /// resolved by the caller, which is where the IAM store is reachable, over the actions
    /// this run CARRIES OUT ([`plan_run`] is how the caller selects them), and decides the
    /// terminal status the run earns — see [`run_plan`].
    pub fn from_template(
        id: String,
        arn: String,
        template: &ExperimentTemplateRecord,
        actions_mode: &str,
        tags: Option<HashMap<String, String>>,
        now: SystemTime,
        failure: Option<ExperimentFailure>,
    ) -> Self {
        let targets = experiment_targets(&template.targets);
        // Resolution happens ONCE, here, where the experiment is started.
        let resolved_targets = resolve_targets(&targets);
        // What the experiment options make of that resolution and of the template's
        // actions. The same plan the caller decided the role's permissions over, computed
        // from the same template and mode — see [`plan_run`].
        let plan = run_actions(
            &template.actions,
            actions_mode,
            template
                .experiment_options
                .empty_target_resolution_mode
                .as_deref(),
            &empty_target_names(&targets, &resolved_targets),
        );
        // An empty resolution is decided BEFORE any action runs, so it is the failure such
        // a run ends in; `failure` is what the start showed about the actions it carries
        // out, and under `skip-all` there are none for it to be about.
        let (run_span, outcome) = run_plan(
            &plan.carried_out,
            plan.empty_target_failure.clone().or(failure),
        );
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
                .map(|(name, action)| {
                    (
                        name.clone(),
                        experiment_action(action, plan.skipped.get(name)),
                    )
                })
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
            run_span,
            outcome,
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
    /// `initiating` when [`TRANSITION_DELAY`] elapsed is `running` from that instant, one
    /// that was `running` when its [`run_span`](Self::run_span) elapsed carries its
    /// [`outcome`](Self::outcome) from that instant, and one that was `stopping` is
    /// `stopped`. Nothing here depends on the call being made, so two reads at the same
    /// instant report the same status.
    ///
    /// The loop is what makes a single read materialise a CHAIN of transitions that all
    /// fell due before `now` — an experiment nobody polled for longer than its whole run
    /// reports the terminal status, not the `running` it passed through.
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(due) = self.transition_due {
            if now < due {
                return;
            }
            match self.status {
                ExperimentStatus::Initiating => {
                    self.status = ExperimentStatus::Running;
                    // The run started when the transition fell due, so what is left of it
                    // is measured from there and not from this call.
                    self.transition_due = Some(due + self.run_span);
                }
                ExperimentStatus::Running => {
                    match &self.outcome {
                        ExperimentOutcome::Completed => {
                            self.status = ExperimentStatus::Completed;
                            self.reason = Some(COMPLETED_REASON.to_string());
                        }
                        ExperimentOutcome::Failed(failure) => {
                            self.status = ExperimentStatus::Failed;
                            self.reason = Some(failure.reason.clone());
                        }
                    }
                    // The experiment ended when the transition fell due, not when somebody
                    // happened to look.
                    self.end_time = Some(due);
                    self.transition_due = None;
                }
                ExperimentStatus::Stopping => {
                    self.status = ExperimentStatus::Stopped;
                    self.end_time = Some(due);
                    self.transition_due = None;
                }
                ExperimentStatus::Stopped
                | ExperimentStatus::Completed
                | ExperimentStatus::Failed => self.transition_due = None,
            }
        }
    }

    /// Start the move to `stopped`, due [`TRANSITION_DELAY`] from `now`.
    ///
    /// A no-op on an experiment that is already stopping or in a final state, so a stop
    /// this service refuses neither restarts the span nor moves `endTime`.
    pub fn request_stop(&mut self, now: SystemTime) {
        if self.status == ExperimentStatus::Stopping || self.status.is_terminal() {
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
            // action has failed", so it is reported exactly when this experiment HAS
            // failed: the failure its run ended in, and nothing at all for a run that is
            // still going, completed or stopped.
            error: match (self.status, &self.outcome) {
                (ExperimentStatus::Failed, ExperimentOutcome::Failed(failure)) => {
                    Some(failure.to_error())
                }
                _ => None,
            },
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

/// What one `@idempotencyToken` created, and the request it created it from.
///
/// The request is kept BESIDE the id because a token identifies a request and not just a
/// resource: replaying it with the same request reports that resource, while reusing it
/// for a different request is a parameter mismatch rather than a retry.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The id of the resource the first call created.
    pub id: String,
    /// The canonical rendering of that first request — see [`template_fingerprint`] and
    /// [`start_experiment_fingerprint`].
    pub request: String,
}

/// What a token the store has already seen says about the request now carrying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReplay {
    /// No live resource was created with this token: carry on and create one.
    Fresh,
    /// The token created this resource from an identical request: report that resource
    /// rather than creating a second one.
    Replay(String),
    /// The token created a resource from a DIFFERENT request. The request is refused with
    /// `ConflictException`; see [`token_conflict`].
    Mismatch,
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
    /// What each `@idempotencyToken` has already created, so replaying a token returns
    /// the first resource instead of minting a second one, and reusing it for another
    /// request is refused.
    ///
    /// An entry is dropped when the resource it names leaves the store, which is what
    /// keeps this map bounded by the live records.
    pub idempotency: BTreeMap<(IdempotentResource, String), IdempotencyRecord>,
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

    /// What `token` says about a request whose canonical rendering is `request`.
    ///
    /// A token whose resource has since left the store is [`TokenReplay::Fresh`]: there
    /// is nothing left to report, so the token may create again.
    pub fn replayed(
        &self,
        resource: IdempotentResource,
        token: &str,
        request: &str,
    ) -> TokenReplay {
        let Some(record) = self.idempotency.get(&(resource, token.to_string())) else {
            return TokenReplay::Fresh;
        };
        let live = match resource {
            IdempotentResource::ExperimentTemplate => {
                self.experiment_templates.contains_key(&record.id)
            }
            IdempotentResource::Experiment => self.experiments.contains_key(&record.id),
        };
        if !live {
            return TokenReplay::Fresh;
        }
        if record.request != request {
            return TokenReplay::Mismatch;
        }
        TokenReplay::Replay(record.id.clone())
    }

    /// Record that `token`, carrying the request `request`, created `id`.
    pub fn remember_token(
        &mut self,
        resource: IdempotentResource,
        token: &str,
        id: &str,
        request: &str,
    ) {
        self.idempotency.insert(
            (resource, token.to_string()),
            IdempotencyRecord {
                id: id.to_string(),
                request: request.to_string(),
            },
        );
    }

    /// Drop every idempotency entry that names `id`, so the map does not outlive the
    /// records it points at.
    pub fn forget_tokens_for(&mut self, resource: IdempotentResource, id: &str) {
        self.idempotency
            .retain(|(kind, _), created| *kind != resource || created.id != id);
    }

    /// How many experiments still occupy an active slot at `now`.
    ///
    /// Call it on a SETTLED store ([`settle_experiments`](Self::settle_experiments)), so
    /// an experiment whose run has already elapsed does not hold a slot nobody is using.
    pub fn active_experiments(&self) -> usize {
        self.experiments
            .values()
            .filter(|record| record.status.is_active())
            .count()
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

/// The message FIS refuses a `roleArn` that does not belong to the `accountId` it is being
/// configured for with.
///
/// `RoleArn` is modelled loosely (`@length(20..2048)`, `pattern ^[\S]+$`), so the rule is
/// a server-side one and its wording is FIS's: the refusal names the parameter and the
/// relationship it states, and carries neither the role ARN nor the account id (the
/// identical text is recorded for different values). See
/// [`target_account_role_arn_refusal`] for the rule itself.
pub const TARGET_ACCOUNT_ROLE_NOT_OWNED: &str = "The roleArn parameter value is not valid. \
     The IAM role must belong to the account specified in the accountId parameter value.";

/// `None` when `role_arn` is a role of the account `account_id` names; otherwise the
/// `ValidationException` that refuses it.
///
/// A target account configuration says WHICH role FIS assumes in ONE account, so the two
/// members have to agree: the account field of the `roleArn` is the account the role
/// belongs to, and a role belonging to any other account is refused with a 400
/// `ValidationException` before anything is persisted — a rule the model cannot express
/// and which therefore lives server-side. Every member of this service that names a role
/// FOR a given account goes through here (`CreateTargetAccountConfiguration` and
/// `UpdateTargetAccountConfiguration`), and the ARN's own syntax
/// ([`role_arn_refusal`]) is checked with it, so a value that is no ARN at all — which
/// names no account to compare — is still refused, by the rule that fits it.
///
/// The ownership comparison is only MADE for an `accountId` that is an AWS account number:
/// `TargetAccountId` is modelled `@length(12..48)`, so a value inside those bounds that is
/// not an account id names no account for a role to belong to, and refusing THAT value is
/// the business of the rule on `accountId` itself rather than of this one.
pub fn target_account_role_arn_refusal(
    account_id: &str,
    role_arn: Option<&str>,
) -> Option<ValidationException> {
    let role_arn = role_arn?;
    let configured_account = is_account_id(account_id);
    let named_account = ::service_common::arn::parse(role_arn).map(|arn| arn.account);
    if let Some(named_account) = named_account {
        if configured_account && named_account != account_id {
            return Some(invalid_input(TARGET_ACCOUNT_ROLE_NOT_OWNED));
        }
    }
    role_arn_refusal("roleArn", Some(role_arn))
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
///
/// This one FIS builds from the request: the message names the id that was looked up.
pub fn experiment_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Experiment with ID {id} not found.")),
    }
}

/// The `ConflictException` payload for an `@idempotencyToken` replayed with a request that
/// is not the one it created its resource from.
pub fn token_conflict(token: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "clientToken {token} is not assigned to this resource."
        )),
    }
}

/// The `ValidationException` payload for a call that would move an experiment already in
/// a final state. Built from the request: the message names the experiment.
pub fn final_state_refusal(id: &str) -> ValidationException {
    invalid_input(format!(
        "Cannot modify the state of an experiment in final state: {id}."
    ))
}

/// The `ServiceQuotaExceededException` payload for a `StartExperiment` that would push the
/// account past the published active-experiments quota.
pub fn active_quota_exceeded() -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "The number of active experiments cannot exceed {MAX_ACTIVE_EXPERIMENTS}."
        )),
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

/// Why a `duration` an `aws:fis:wait` action carries is not one FIS would accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitDurationProblem {
    /// The value is not an ISO 8601 duration, or names a span whose length is not fixed
    /// (a year or a month).
    NotADuration,
    /// The value is a duration, but outside the documented 1..720 minutes.
    OutOfRange,
}

/// The wait an `aws:fis:wait` action's `duration` parameter names.
///
/// "duration — The duration of the wait" is given in ISO 8601 format ([`iso8601_span`] is
/// the parse) and its documented range is
/// [`WAIT_DURATION_MIN_MINUTES`]..=[`WAIT_DURATION_MAX_MINUTES`] minutes, so a value
/// outside that bound is not a wait this service accepts.
pub fn wait_duration(value: &str) -> Result<Duration, WaitDurationProblem> {
    let span = iso8601_span(value).ok_or(WaitDurationProblem::NotADuration)?;
    let minutes = span.as_secs_f64() / 60.0;
    #[allow(clippy::cast_precision_loss)]
    if minutes < WAIT_DURATION_MIN_MINUTES as f64 || minutes > WAIT_DURATION_MAX_MINUTES as f64 {
        return Err(WaitDurationProblem::OutOfRange);
    }
    Ok(span)
}

/// `None` when every action a `CreateExperimentTemplate` request carries satisfies what
/// FIS documents for it; otherwise the `ValidationException` that refuses the request.
///
/// The rule this enforces is the built-in `aws:fis:wait` action's: it takes a required
/// `duration` parameter, an ISO 8601 duration within the documented 1 to 720 minutes. An
/// action id FIS defines elsewhere carries its own parameter catalog, which this service
/// does not hold, so its parameters are stored as sent. The other rule here is on the
/// graph: `startAfter` is "the name of the action that must be completed before the
/// current action starts", so it names an action the same template declares.
pub fn actions_refusal(
    actions: &HashMap<String, CreateExperimentTemplateActionInput>,
) -> Option<ValidationException> {
    // The map's iteration order is not stable, so the refusal is taken over the action
    // names in order: one request always earns the same message. The entries are sorted
    // as PAIRS, so no path through this function can turn a lookup miss into an
    // acceptance.
    let mut entries: Vec<(&String, &CreateExperimentTemplateActionInput)> =
        actions.iter().collect();
    entries.sort_by_key(|(name, _)| *name);
    for (name, action) in entries {
        if action
            .start_after
            .iter()
            .flatten()
            .any(|earlier| !actions.contains_key(earlier))
        {
            return Some(invalid_input(format!(
                "actions.{name}.startAfter must name actions the template declares."
            )));
        }
        if action.action_id != WAIT_ACTION_ID {
            continue;
        }
        let duration = action
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.get(DURATION_PARAMETER));
        let Some(duration) = duration else {
            return Some(invalid_input(format!(
                "actions.{name}.parameters must carry '{DURATION_PARAMETER}' \
                 for action {WAIT_ACTION_ID}."
            )));
        };
        match wait_duration(duration) {
            Ok(_) => {}
            Err(WaitDurationProblem::OutOfRange) => {
                return Some(invalid_input(format!(
                    "'{DURATION_PARAMETER}' must be within a range of \
                     {WAIT_DURATION_MIN_MINUTES} to {WAIT_DURATION_MAX_MINUTES} minutes."
                )))
            }
            Err(WaitDurationProblem::NotADuration) => {
                return Some(invalid_input(format!(
                    "actions.{name}.parameters.'{DURATION_PARAMETER}' must be a \
                     duration in ISO 8601 format."
                )))
            }
        }
    }
    None
}

/// `None` when a request carries no more actions than one template may hold; otherwise the
/// `ServiceQuotaExceededException` that refuses it.
///
/// See [`MAX_ACTIONS_PER_TEMPLATE`]: the cap is a published quota, and it is also what
/// keeps the per-request cost of [`run_plan`] proportional to the request rather than to
/// its square.
pub fn actions_quota_refusal(
    actions: &HashMap<String, CreateExperimentTemplateActionInput>,
) -> Option<ServiceQuotaExceededException> {
    if actions.len() <= MAX_ACTIONS_PER_TEMPLATE {
        return None;
    }
    Some(ServiceQuotaExceededException {
        message: Some(format!(
            "The number of actions in an experiment template cannot exceed \
             {MAX_ACTIONS_PER_TEMPLATE}."
        )),
    })
}

/// What the experiment options make of a started experiment's actions — see [`RunActions`].
///
/// `actions_mode` is the `actionsMode` `StartExperiment` resolved
/// ([`resolve_actions_mode`]), `empty_target_resolution_mode` is the template's option as
/// stored (absent meaning [`DEFAULT_EMPTY_TARGET_RESOLUTION_MODE`]), and `empty_targets`
/// names the targets that RESOLVED to no resources ([`empty_target_names`]).
///
/// The rules, both of which FIS states for its experiment options:
///
/// * `skip-all` carries out NO action, so none of them is in `carried_out` — which is how
///   a caller checks the target resolution without performing the actions;
/// * an empty resolution is decided before any action runs, so under the mode that fails
///   for it the run ends `failed` on that resolution WHATEVER the actions mode is (a
///   `skip-all` start is exactly the one that has to be able to see it), while under
///   `skip` each action that refers to such a target is skipped and the rest of the
///   experiment runs.
///
/// PURE: nothing here reads the clock, the store or the request — the plan is a function of
/// the template and the mode, so the caller that decides the execution role's permissions
/// over `carried_out` and [`ExperimentRecord::from_template`] agree on it.
///
/// The work is linear in the template: one pass over the actions, each looking at the
/// targets it names, both bounded ([`MAX_ACTIONS_PER_TEMPLATE`]).
pub fn run_actions(
    actions: &HashMap<String, ExperimentTemplateAction>,
    actions_mode: &str,
    empty_target_resolution_mode: Option<&str>,
    empty_targets: &BTreeSet<String>,
) -> RunActions {
    let empty_mode = empty_target_resolution_mode.unwrap_or(DEFAULT_EMPTY_TARGET_RESOLUTION_MODE);
    // The FIRST empty target in name order is the one the failure names, so an experiment
    // started twice from one template fails on the same target both times. The create
    // refused any value the modelled `fail | skip` pair does not admit, so "not skip" is
    // the documented default, `fail`.
    let empty_target_failure = (empty_mode != EMPTY_TARGET_RESOLUTION_MODE_SKIP)
        .then(|| {
            empty_targets
                .first()
                .map(|name| ExperimentFailure::empty_target_resolution(name))
        })
        .flatten();
    let mut carried_out: HashMap<String, ExperimentTemplateAction> =
        HashMap::with_capacity(actions.len());
    let mut skipped: BTreeMap<String, ActionSkip> = BTreeMap::new();
    for (name, action) in actions {
        if actions_mode == ACTIONS_MODE_SKIP_ALL {
            skipped.insert(name.clone(), ActionSkip::AllActions);
            continue;
        }
        // Under `skip` the action an empty target would have been acted on by is the one
        // that is skipped; the targets an action names are its `targets` map's values.
        let empty_target = (empty_mode == EMPTY_TARGET_RESOLUTION_MODE_SKIP)
            .then(|| action_empty_target(action, empty_targets))
            .flatten();
        match empty_target {
            Some(target) => {
                skipped.insert(name.clone(), ActionSkip::EmptyTarget(target));
            }
            None => {
                carried_out.insert(name.clone(), action.clone());
            }
        }
    }
    RunActions {
        carried_out,
        skipped,
        empty_target_failure,
    }
}

/// The plan [`run_actions`] makes for a run `StartExperiment` is about to persist for
/// `template` under `actions_mode`.
///
/// What the start itself needs it for is `carried_out`: the actions the execution role's
/// permissions are decided over, which it has to know BEFORE the record exists. It is the
/// same plan [`ExperimentRecord::from_template`] then computes for the record, because both
/// are this pure function of the same template and the same mode.
pub fn plan_run(template: &ExperimentTemplateRecord, actions_mode: &str) -> RunActions {
    let targets = experiment_targets(&template.targets);
    let resolved = resolve_targets(&targets);
    run_actions(
        &template.actions,
        actions_mode,
        template
            .experiment_options
            .empty_target_resolution_mode
            .as_deref(),
        &empty_target_names(&targets, &resolved),
    )
}

/// The target `action` refers to that resolved to no resources, taken in the order of the
/// action's own target KEYS so one action always reports the same one.
fn action_empty_target(
    action: &ExperimentTemplateAction,
    empty_targets: &BTreeSet<String>,
) -> Option<String> {
    let named: BTreeMap<&str, &str> = action
        .targets
        .iter()
        .flatten()
        .map(|(key, name)| (key.as_str(), name.as_str()))
        .collect();
    named
        .values()
        .find(|name| empty_targets.contains(**name))
        .map(|name| (*name).to_string())
}

/// The names of the targets that RESOLVED to no resources — the condition
/// `emptyTargetResolutionMode` decides on.
///
/// Read off the resolution itself rather than recomputed, so what
/// `ListExperimentResolvedTargets` reports for an experiment and what its run made of an
/// empty target are the SAME fact: a target no entry of the resolution names resolved to
/// nothing (a target identified by `resourceTags`, whose resources belong to services this
/// emulator does not hold, is the case that reaches here — see [`resolve_targets`]).
pub fn empty_target_names(
    targets: &HashMap<String, ExperimentTarget>,
    resolved: &[ResolvedTarget],
) -> BTreeSet<String> {
    let named: HashSet<&str> = resolved
        .iter()
        .filter_map(|entry| entry.target_name.as_deref())
        .collect();
    targets
        .keys()
        .filter(|name| !named.contains(name.as_str()))
        .cloned()
        .collect()
}

/// How long the actions an experiment CARRIES OUT take once it is `running`, and what the
/// run ends as.
///
/// `actions` is the actions the run carries out — [`RunActions::carried_out`], which is
/// every action of the template unless an experiment option skipped one. An action the run
/// does not carry out is not among them and so spends none of its duration.
///
/// FIS starts an action as soon as every action its `startAfter` names has finished, so a
/// run lasts as long as the longest path through that graph ([`critical_path`]), which is
/// then compressed onto the emulator's run clock ([`run_span`]). An experiment that is
/// started RUNS: it is observable in `running` for that span and then reaches the terminal
/// status its actions earned.
///
/// `failure` is what the start showed about the run before it began — the account's IAM
/// store showing the experiment's execution role is not allowed to perform an action's own
/// API calls, or a target of the experiment having resolved to no resources under the
/// `emptyTargetResolutionMode` that fails for it. Neither run gets going, so it claims no
/// duration at all: it is one transition span long and ends `failed`, carrying the failure,
/// which is the status and the attribution FIS reports for it. With nothing shown against
/// it the run ends `completed`.
///
/// The span is never zero: a caller polling after `StartExperiment` observes `initiating`
/// and then `running`, exactly as it would against the real service.
pub fn run_plan(
    actions: &HashMap<String, ExperimentTemplateAction>,
    failure: Option<ExperimentFailure>,
) -> (Duration, ExperimentOutcome) {
    match failure {
        Some(failure) => (TRANSITION_DELAY, ExperimentOutcome::Failed(failure)),
        None => (
            run_span(critical_path(actions)),
            ExperimentOutcome::Completed,
        ),
    }
}

/// `modelled` on the emulator's run clock: scaled by [`RUN_CLOCK_SCALE`] and clamped into
/// [`MIN_RUN_SPAN`]..=[`MAX_RUN_SPAN`].
///
/// Monotonic in `modelled` — a longer modelled run is never a shorter real one — so two
/// experiments ordered by the durations their actions name stay ordered by the spans they
/// run for, up to the ceiling.
pub fn run_span(modelled: Duration) -> Duration {
    (modelled / RUN_CLOCK_SCALE).clamp(MIN_RUN_SPAN, MAX_RUN_SPAN)
}

/// The longest path through `actions`, where each action's own length is the `duration` it
/// was asked for and an action that names none is instantaneous.
///
/// Walked ONCE per action, with each action's finish offset memoised and its own span
/// parsed exactly once up front, so the whole computation costs O(actions + `startAfter`
/// entries) — linear in the request, never in its square. The number of actions is capped
/// at [`MAX_ACTIONS_PER_TEMPLATE`] when the template is created, which also bounds the
/// recursion depth here; a `startAfter` cycle, which FIS refuses but which a record could
/// still carry, is broken at the back edge rather than recursing forever.
fn critical_path(actions: &HashMap<String, ExperimentTemplateAction>) -> Duration {
    // Parsed once per action, outside the walk.
    let spans: HashMap<&str, Duration> = actions
        .iter()
        .map(|(name, action)| (name.as_str(), action_span(action)))
        .collect();
    let mut finish: HashMap<&str, Duration> = HashMap::with_capacity(actions.len());
    let mut longest = Duration::ZERO;
    for name in actions.keys() {
        let mut walking: Vec<&str> = Vec::new();
        let offset = finish_offset(name.as_str(), actions, &spans, &mut finish, &mut walking);
        longest = longest.max(offset);
    }
    longest
}

/// When `name` finishes, measured from the start of the run: the latest finish of the
/// actions its `startAfter` names, plus its own span.
///
/// `finish` memoises the answer for every action already resolved, and `walking` holds the
/// chain currently being resolved so a cycle is answered as "no dependency" instead of
/// recursing. See [`critical_path`] for the cost this keeps.
fn finish_offset<'a>(
    name: &'a str,
    actions: &'a HashMap<String, ExperimentTemplateAction>,
    spans: &HashMap<&'a str, Duration>,
    finish: &mut HashMap<&'a str, Duration>,
    walking: &mut Vec<&'a str>,
) -> Duration {
    if let Some(known) = finish.get(name) {
        return *known;
    }
    if walking.contains(&name) {
        return Duration::ZERO;
    }
    walking.push(name);
    let mut start = Duration::ZERO;
    if let Some(action) = actions.get(name) {
        for earlier in action.start_after.iter().flatten() {
            // `startAfter` names an action the template declares (`actions_refusal`); a
            // name the map does not hold contributes nothing rather than a guess.
            if let Some((key, _)) = actions.get_key_value(earlier.as_str()) {
                start = start.max(finish_offset(key.as_str(), actions, spans, finish, walking));
            }
        }
    }
    walking.pop();
    // SATURATING: a template may name a `duration` of its own choosing for any action
    // other than `aws:fis:wait` (whose range `actions_refusal` checks), so a chain of them
    // can add up past what a `Duration` holds. The sum is capped there rather than
    // overflowing — a run that long is clamped to [`MAX_RUN_SPAN`] either way.
    let total = start.saturating_add(spans.get(name).copied().unwrap_or(Duration::ZERO));
    finish.insert(name, total);
    total
}

/// How long one action takes: the span its `duration` parameter names, and nothing at all
/// for an action that carries none.
///
/// SOURCE: the AWS FIS actions reference, where `duration` is "the duration of the action"
/// (in ISO 8601 format) for every built-in action that takes it — `aws:fis:wait`, the
/// `aws:fis:inject-api-*` family, `aws:ebs:pause-volume-io`,
/// `aws:network:disrupt-connectivity`, … — so the parameter is read by its documented
/// meaning rather than for one action id. An action whose effect is a single API call
/// (`aws:ec2:reboot-instances`) takes no `duration` and so names no span; a parameter that
/// is some other span (`aws:ec2:stop-instances`' `startInstancesAfterDuration`) is not
/// this one and is not read as it.
///
/// A value that is not an ISO 8601 duration contributes nothing rather than a made-up
/// span: [`wait_duration`]'s own 1..720-minute range is NOT applied here, because that is
/// the range documented for `aws:fis:wait` and each other action publishes its own.
fn action_span(action: &ExperimentTemplateAction) -> Duration {
    action
        .parameters
        .as_ref()
        .and_then(|parameters| parameters.get(DURATION_PARAMETER))
        .and_then(|duration| iso8601_span(duration))
        .unwrap_or(Duration::ZERO)
}

/// The fixed span the ISO 8601 duration `value` names, or `None` when it names none.
///
/// A duration whose length depends on the calendar (a year or a month designator) names no
/// fixed span, so it is read as no duration at all rather than guessed at — the same
/// reading [`wait_duration`], which is this parse under `aws:fis:wait`'s documented range,
/// takes of one.
///
/// TOTAL over every string a caller can send: a value this cannot read as a fixed,
/// representable span — one that is not a duration, one that is negative, and one whose
/// span does not fit a [`Duration`] (the parser's fields are floats, so a request may name
/// 1e20 seconds) — is `None`. Nothing here may panic: the `duration` of an action that is
/// not `aws:fis:wait` is never range-checked when the template is created, so this parse
/// runs on unvalidated request text inside `StartExperiment`, while that handler holds the
/// store guard.
pub fn iso8601_span(value: &str) -> Option<Duration> {
    let parsed = ::iso8601_duration::Duration::parse(value).ok()?;
    if parsed.year != 0.0 || parsed.month != 0.0 {
        return None;
    }
    let seconds = f64::from(parsed.day) * 86_400.0
        + f64::from(parsed.hour) * 3_600.0
        + f64::from(parsed.minute) * 60.0
        + f64::from(parsed.second);
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    // `try_from_secs_f64` is the non-panicking construction: a value a `Duration` cannot
    // hold names no span this service can run, exactly like a value that is no duration.
    Duration::try_from_secs_f64(seconds).ok()
}

/// `None` when this emulator has no reason to think FIS could not assume `role_arn`;
/// otherwise the `ValidationException` that refuses the call.
///
/// FIS runs an experiment under the template's execution role, so it checks up front that
/// the role's trust policy lets `fis.amazonaws.com` assume it and refuses the start when
/// it does not. The role is resolved against the account's IAM records — the same records
/// `CreateRole` wrote — by its ARN. An ARN the IAM store holds no role for is NOT evidence
/// of a missing trust (the role may belong to another account, or to a client that never
/// told this emulator about it), so the start goes ahead; only a role this emulator holds
/// and whose trust policy withholds the grant is refused.
pub fn role_trust_refusal(role_arn: &str, iam: &IamState) -> Option<ValidationException> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    match policy_grants_service(document, FIS_SERVICE_PRINCIPAL) {
        // A document this emulator cannot read as a policy is no evidence either way.
        None | Some(true) => None,
        Some(false) => Some(invalid_input(format!(
            "Unable to assume role {role_arn}. Check the trust policy grants access to \
             '{FIS_SERVICE_PRINCIPAL}'"
        ))),
    }
}

/// Whether `document` — an IAM trust policy — lets the service principal `principal`
/// assume the role, or `None` when it is not a policy this can be read out of.
///
/// The rule itself is IAM's, not this service's, so it is read through the ONE evaluator
/// every service that resolves a caller-supplied execution role shares
/// (`service_common::iam_trust_policy`): the statement must name the principal and the
/// assume-role action, an explicit `Deny` wins over any `Allow`, and the document is
/// percent-decoded before it is parsed. What to do when it cannot be read as a policy is
/// this service's own choice, which [`role_trust_refusal`] makes.
fn policy_grants_service(document: &str, principal: &str) -> Option<bool> {
    ::service_common::iam_trust_policy::admits_service_principal(document, principal)
}

/// `map` as an ordered rendering of its entries, or `null` when it is absent.
///
/// Every map that goes into a fingerprint goes through here. A `HashMap`'s iteration order
/// is NOT stable — two maps holding the same entries can walk them in different orders —
/// and `serde_json`'s object type preserves insertion order verbatim in any build that
/// resolves the crate's `preserve_order` feature, so an unsorted map would render one
/// request two ways and make a retry look like a different request. Sorting the entries
/// here is what makes the rendering canonical under EITHER object type.
fn ordered_entries(map: Option<&HashMap<String, String>>) -> ::serde_json::Value {
    match map {
        Some(map) => {
            let ordered: BTreeMap<&str, &str> = map
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            ::serde_json::json!(ordered)
        }
        None => ::serde_json::Value::Null,
    }
}

/// The canonical rendering of the `StartExperiment` request a `clientToken` belongs to.
///
/// Every member of the request that chooses what gets started, and nothing else: two
/// requests with the same rendering start the same experiment, so one is a retry of the
/// other. Every map is rendered in key order (see [`ordered_entries`]), so a map's
/// iteration order cannot make one request look like two.
pub fn start_experiment_fingerprint(input: &StartExperimentInput) -> String {
    ::serde_json::json!({
        "experimentTemplateId": input.experiment_template_id,
        "actionsMode": input
            .experiment_options
            .as_ref()
            .and_then(|options| options.actions_mode.clone()),
        "tags": ordered_entries(input.tags.as_ref()),
    })
    .to_string()
}

/// The canonical rendering of the `CreateExperimentTemplate` request a `clientToken`
/// belongs to, taken from the record that request builds.
///
/// Rendered from the RECORD rather than the request so every member the service persists
/// is covered, including the defaults it filled in; the minted id, the ARN and the
/// timestamps are left out because they are what the call produces rather than what it
/// asked for. See [`start_experiment_fingerprint`] for why the rendering is JSON.
pub fn template_fingerprint(record: &ExperimentTemplateRecord) -> String {
    ::serde_json::json!({
        "description": record.description,
        "roleArn": record.role_arn,
        "tags": ordered_entries(Some(&record.tags)),
        "stopConditions": record
            .stop_conditions
            .iter()
            .map(|condition| ::serde_json::json!({
                "source": condition.source,
                "value": condition.value,
            }))
            .collect::<Vec<_>>(),
        "targets": record
            .targets
            .iter()
            .map(|(name, target)| (name.as_str(), ::serde_json::json!({
                "resourceType": target.resource_type,
                "resourceArns": target.resource_arns,
                "resourceTags": ordered_entries(target.resource_tags.as_ref()),
                "selectionMode": target.selection_mode,
                "parameters": ordered_entries(target.parameters.as_ref()),
                "filters": target.filters.as_ref().map(|filters| filters
                    .iter()
                    .map(|filter| ::serde_json::json!({
                        "path": filter.path,
                        "values": filter.values,
                    }))
                    .collect::<Vec<_>>()),
            })))
            .collect::<BTreeMap<&str, ::serde_json::Value>>(),
        "actions": record
            .actions
            .iter()
            .map(|(name, action)| (name.as_str(), ::serde_json::json!({
                "actionId": action.action_id,
                "description": action.description,
                "parameters": ordered_entries(action.parameters.as_ref()),
                "targets": ordered_entries(action.targets.as_ref()),
                "startAfter": action.start_after,
            })))
            .collect::<BTreeMap<&str, ::serde_json::Value>>(),
        "logConfiguration": record.log_configuration.as_ref().map(|logs| ::serde_json::json!({
            "logSchemaVersion": logs.log_schema_version,
            "cloudWatchLogsConfiguration": logs
                .cloud_watch_logs_configuration
                .as_ref()
                .map(|configuration| configuration.log_group_arn.clone()),
            "s3Configuration": logs.s3_configuration.as_ref().map(|s3| ::serde_json::json!({
                "bucketName": s3.bucket_name,
                "prefix": s3.prefix,
            })),
        })),
        "experimentOptions": {
            "accountTargeting": record.experiment_options.account_targeting,
            "emptyTargetResolutionMode": record.experiment_options.empty_target_resolution_mode,
        },
        "experimentReportConfiguration": record
            .experiment_report_configuration
            .as_ref()
            .map(|report| ::serde_json::json!({
                "preExperimentDuration": report.pre_experiment_duration,
                "postExperimentDuration": report.post_experiment_duration,
                "outputs": report.outputs.as_ref().and_then(|outputs| outputs
                    .s3_configuration
                    .as_ref()
                    .map(|s3| ::serde_json::json!({
                        "bucketName": s3.bucket_name,
                        "prefix": s3.prefix,
                    }))),
                "dataSources": report.data_sources.as_ref().and_then(|sources| sources
                    .cloud_watch_dashboards
                    .as_ref()
                    .map(|dashboards| dashboards
                        .iter()
                        .map(|dashboard| dashboard.dashboard_identifier.clone())
                        .collect::<Vec<_>>())),
            })),
    })
    .to_string()
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

/// The experiment's copy of the template's targets, which is also what the resolution and
/// the empty-target rule are taken over ([`resolve_targets`], [`empty_target_names`]).
fn experiment_targets(
    targets: &HashMap<String, ExperimentTemplateTarget>,
) -> HashMap<String, ExperimentTarget> {
    targets
        .iter()
        .map(|(name, target)| (name.clone(), experiment_target(target)))
        .collect()
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

/// The experiment's copy of one of the template's actions, with the `state` the run's plan
/// gives it: `skip` is `Some` exactly for an action the run does NOT carry out.
///
/// `startTime` and `endTime` are left unset on purpose, and so is `state` for an action the
/// run carries out: they report the progress of ONE action, and what this emulator models
/// is the progress of the EXPERIMENT (its `ExperimentState` and the run span the actions it
/// carries out add up to), not the effect each action has on another service — so no
/// per-action status is claimed for work this emulator did not carry out. Reporting them is
/// the business of whatever models those effects.
///
/// A SKIPPED action is the one action whose state is known without any work having been
/// done: the experiment options decided it would not be carried out, which is what the
/// modelled `skipped` status says ([`ActionSkip::to_state`]). Its `startTime` and `endTime`
/// stay unset, because it never ran.
fn experiment_action(
    action: &ExperimentTemplateAction,
    skip: Option<&ActionSkip>,
) -> ExperimentAction {
    ExperimentAction {
        action_id: action.action_id.clone(),
        description: action.description.clone(),
        parameters: action.parameters.clone(),
        targets: action.targets.clone(),
        start_after: action.start_after.clone(),
        state: skip.map(ActionSkip::to_state),
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
    /// The IAM actions the experiment's execution role has to be allowed to perform for
    /// this action to run, empty for an action that calls no other service's API.
    ///
    /// NOT part of the `Action` shape — `GetAction` reports no permissions — but part of
    /// what running the action MEANS: FIS runs it under the template's `roleArn`, so an
    /// action whose API calls that role is not allowed to make cannot be carried out (see
    /// [`action_permissions`]). The primary API call of each action, as the FIS actions
    /// reference states it for that action; an action this table records no permission for
    /// is never failed for privileges.
    pub permissions: &'static [&'static str],
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

/// The IAM action the `aws:fis:inject-api-*` family and the EC2 API-error actions need:
/// the FIS actions reference states `ec2:InjectApiError` for them, scoped with the
/// `ec2:FisActionId` condition key — which is why the gate decides with that key in the
/// request's condition context (see [`action_permissions`]), so a policy written the way
/// the reference writes it grants the action it names and only that action.
pub const EC2_INJECT_API_ERROR: &str = "ec2:InjectApiError";

/// The condition key the documented `ec2:InjectApiError` grant is scoped by: "the FIS
/// action id", which is the value the experiment's action carries.
pub const FIS_ACTION_ID_CONDITION_KEY: &str = "ec2:FisActionId";

/// The built-in FIS actions this emulator serves.
///
/// SOURCE: the AWS FIS actions reference
/// (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
/// `Action`'s own model documentation points at ("Describes an action. For more
/// information, see FIS actions in the Fault Injection Service User Guide"). Each entry
/// records what that reference states for the action: its id, a one-line summary of what
/// it does, the parameters it accepts and which of them are required, the target
/// resource types it supports under the key an experiment template refers to them by, and
/// the IAM action its own API call needs ("the IAM permissions required" the reference
/// lists for each action), which the experiment's execution role has to be allowed to
/// perform for the action to run.
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
        permissions: &["cloudwatch:DescribeAlarms"],
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
        permissions: &["ec2:PauseVolumeIO"],
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
        permissions: &[EC2_INJECT_API_ERROR],
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
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:ec2:reboot-instances",
        description: "Reboot the target EC2 instances.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
        permissions: &["ec2:RebootInstances"],
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
        permissions: &["ec2:SendSpotInstanceInterruptions"],
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
        permissions: &["ec2:StopInstances"],
    },
    ActionCatalogEntry {
        id: "aws:ec2:terminate-instances",
        description: "Terminate the target EC2 instances.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
        permissions: &["ec2:TerminateInstances"],
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
        permissions: &["ecs:UpdateContainerInstancesState"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:stop-task",
        description: "Stop the target Amazon ECS tasks.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ecs:StopTask"],
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
        permissions: &["ec2:TerminateInstances"],
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
        permissions: &[EC2_INJECT_API_ERROR],
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
        permissions: &[EC2_INJECT_API_ERROR],
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
        permissions: &[EC2_INJECT_API_ERROR],
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
        // The wait calls no other service's API, so the execution role needs no
        // permission for it: it is the one action whose whole effect is elapsed time.
        permissions: &[],
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
        permissions: &["ec2:CreateNetworkAcl"],
    },
    ActionCatalogEntry {
        id: "aws:rds:failover-db-cluster",
        description: "Fail over the target Amazon Aurora DB clusters.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Clusters",
            resource_type: "aws:rds:cluster",
        }],
        permissions: &["rds:FailoverDBCluster"],
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
        permissions: &["rds:RebootDBInstance"],
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
        permissions: &["ssm:SendCommand"],
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
        permissions: &["ssm:StartAutomationExecution"],
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

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied, so FIS reads the
/// very store that service's handlers write. `None` has the same meaning as for
/// [`service_state`]: the engine carries no such store, which a handler reports as a
/// server fault.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}
