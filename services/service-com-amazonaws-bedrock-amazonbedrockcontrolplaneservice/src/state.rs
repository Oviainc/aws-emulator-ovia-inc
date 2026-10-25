// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrock-amazonbedrockcontrolplaneservice service.
//!
//! Today this holds the guardrail resource: the control plane's self-contained
//! CRUD resource, owned by `ops/{create,get,list,update,delete}_guardrail.rs` and
//! `ops/create_guardrail_version.rs`. Every one of those handlers reads and writes
//! THIS struct through [`service_state`], so a guardrail created by one request is
//! the guardrail every later request sees.
//!
//! It also holds the account's model invocation logging configuration — the single
//! settings record `ops/{get,put,delete}_model_invocation_logging_configuration.rs`
//! share, so the configuration one request stores is the configuration the next
//! request reads back.
//!
//! It also holds the account's enforced guardrail configuration — the single record
//! `ops/put_enforced_guardrail_configuration.rs` installs and
//! `ops/list_enforced_guardrails_configuration.rs` reports, so the configuration one
//! request stores is the configuration the next request lists.
//!
//! And it holds the prompt routers `ops/create_prompt_router.rs` mints and
//! `ops/get_prompt_router.rs` reads back, so a router created by one request is the
//! router every later request sees.
//!
//! And it holds the model import jobs `ops/create_model_import_job.rs` mints and
//! `ops/get_model_import_job.rs` reads back, together with the instant each job's
//! non-terminal `status` is owed its move to `Completed`. Those same records hold the
//! models those jobs import, which `ops/get_imported_model.rs` reads back once the
//! import has completed — the job that imports a model is where that model's identity,
//! data source and key live, so there is only ever one copy of either.
//!
//! And it holds the evaluation jobs `ops/create_evaluation_job.rs` mints and
//! `ops/get_evaluation_job.rs` reads back.
//!
//! And it holds the Bedrock Marketplace model endpoints
//! `ops/create_marketplace_model_endpoint.rs` mints,
//! `ops/update_marketplace_model_endpoint.rs` reconfigures and
//! `ops/get_marketplace_model_endpoint.rs` reads back, together with the instant each
//! endpoint's non-terminal `endpointStatus` is owed its move to `InService`.
//!
//! And it holds the Provisioned Throughputs `ops/create_provisioned_model_throughput.rs`
//! mints, `ops/get_provisioned_model_throughput.rs` reads back,
//! `ops/list_provisioned_model_throughputs.rs` reports and
//! `ops/delete_provisioned_model_throughput.rs` removes, together with the instant each
//! one's non-terminal `status` is owed its move to `InService`.
//!
//! And it holds the custom models `ops/create_custom_model.rs` mints,
//! `ops/get_custom_model.rs` reads back and `ops/list_custom_models.rs` reports,
//! together with the instant each one's non-terminal `modelStatus` is owed its move to
//! `Active`.
//!
//! And it holds the account's acknowledged use case for model access — the single form
//! `ops/put_use_case_for_model_access.rs` submits and `ops/get_use_case_for_model_access.rs`
//! reads back, so the form one request submits is the form the next request gets.
//!
//! And it holds the published foundation model catalog — the base models
//! `ops/list_foundation_models.rs` reports — which is seeded with the store and never
//! written, because no operation of this service creates a foundation model.
//!
//! Tagging adds NO store of its own: `ops/tag_resource.rs` writes, and
//! `ops/list_tags_for_resource.rs` reports, the `tags` list of the record that already
//! holds the resource a `resourceARN` names — the same list the create that minted the
//! resource filled — so there is only ever one copy of a resource's tags, whichever
//! operation reads them. [`TaggedResource`] is what a `resourceARN` resolves to, and
//! resolving it is what keeps every tagging operation addressing the same record.
//!
//! The record shapes, and the conversions between the `*PolicyConfig` shapes a
//! request carries and the `*Policy` shapes a response carries, live here rather
//! than in one operation's file because six operations share them (a file under
//! `src/ops/` is claimed by the build as an operation, so a shared helper cannot
//! live there).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CustomModelDataSource, CustomModelSummary, EndpointConfig, EvaluationConfig,
    EvaluationInferenceConfig, EvaluationOutputDataConfig,
    GuardrailAutomatedReasoningPolicy, GuardrailAutomatedReasoningPolicyConfig,
    GuardrailContentFilter, GuardrailContentFilterConfig, GuardrailContentFiltersTier,
    GuardrailContentFiltersTierConfig, GuardrailContentPolicy, GuardrailContentPolicyConfig,
    GuardrailContextualGroundingFilter, GuardrailContextualGroundingFilterConfig,
    GuardrailContextualGroundingPolicy, GuardrailContextualGroundingPolicyConfig,
    GuardrailContextualGroundingTier, GuardrailContextualGroundingTierConfig,
    GuardrailCrossRegionConfig, GuardrailCrossRegionDetails, GuardrailManagedWords,
    GuardrailManagedWordsConfig, GuardrailPiiEntity, GuardrailPiiEntityConfig, GuardrailRegex,
    GuardrailRegexConfig, GuardrailSensitiveInformationPolicy,
    GuardrailSensitiveInformationPolicyConfig, GuardrailTopic, GuardrailTopicConfig,
    GuardrailTopicPolicy, GuardrailTopicPolicyConfig, GuardrailTopicsTier,
    GuardrailTopicsTierConfig, GuardrailWord, GuardrailWordConfig, GuardrailWordPolicy,
    GuardrailWordPolicyConfig, LoggingConfig, MarketplaceModelEndpoint, ModelDataSource,
    ModelEnforcement, PromptRouterTargetModel, ProvisionedModelSummary, RoutingCriteria,
    SelectiveContentGuarding, Tag, VpcConfig,
};

/// The working version every guardrail always has, as `GuardrailDraftVersion`
/// (`^DRAFT$`) spells it.
pub const DRAFT_VERSION: &str = "DRAFT";

/// `GuardrailStatus` — a guardrail that is serving. The status every revision this
/// emulator mints carries, from the call that mints it onwards.
///
/// `CreateGuardrail`, `UpdateGuardrail` and `CreateGuardrailVersion` return no status
/// member at all, and the first read that follows any of them reports `READY`, so a
/// guardrail is never observed in a non-terminal status: there is no transition for the
/// clock to owe, and nothing asynchronous for this emulator to wait on. The other
/// `GuardrailStatus` values (`CREATING`, `UPDATING`, `VERSIONING`, `DELETING`, `FAILED`)
/// name states no call here produces, so none of them is minted.
pub const STATUS_READY: &str = "READY";

/// `GuardrailContentFiltersTierName` / `GuardrailTopicsTierName` /
/// `GuardrailContextualGroundingTierName` — the tier a policy block reports when the
/// request configured none.
///
/// All three enums are `[CLASSIC, STANDARD]`, and the model documents `STANDARD` as the
/// opt-in tier that "requires that your guardrail use cross-Region inference" while
/// `CLASSIC` "provides established guardrails functionality" — so `CLASSIC` is the tier a
/// guardrail is on until a request names the other one.
pub const TIER_CLASSIC: &str = "CLASSIC";

/// `ConfigurationOwner` — who owns an enforced guardrail configuration.
///
/// The enum carries exactly one value, `ACCOUNT`
/// (`smithy_query(query="shape",shape="ConfigurationOwner")`), and the configuration the
/// put installs is the ACCOUNT-level one, so this is the owner every stored configuration
/// reports.
pub const CONFIGURATION_OWNER_ACCOUNT: &str = "ACCOUNT";

/// The most guardrails this account may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// The Amazon Bedrock service quota "Guardrails per account" has a default of 100
/// (adjustable); this emulator enforces that default, so the store cannot grow
/// without a server-side bound no matter how many creates a caller issues.
pub const MAX_GUARDRAILS_PER_ACCOUNT: usize = 100;

/// The most numbered versions one guardrail may hold at once, past which cutting
/// another is refused with `ServiceQuotaExceededException`.
///
/// The Amazon Bedrock service quota "Versions per guardrail" has a default of 20
/// (adjustable); this emulator enforces that default, so the snapshots one
/// guardrail accumulates are bounded too.
pub const MAX_VERSIONS_PER_GUARDRAIL: usize = 20;

/// The largest version number `GuardrailNumericalVersion` (`^[1-9][0-9]{0,7}$`) can
/// spell. Version numbers are never reused, so a guardrail that has cut and deleted
/// this many versions can mint no more and `CreateGuardrailVersion` refuses rather
/// than minting a number the model does not admit.
pub const MAX_VERSION_NUMBER: u32 = 99_999_999;

/// `PromptRouterStatus` — a prompt router that is serving.
///
/// `AVAILABLE` is the ONLY value the enum carries
/// (`smithy_query(query="shape",shape="PromptRouterStatus")`), so a router this emulator
/// mints is serving from the call that mints it onwards: there is no non-terminal status
/// for the clock to move it out of, and nothing asynchronous to wait on.
pub const PROMPT_ROUTER_AVAILABLE: &str = "AVAILABLE";

/// `PromptRouterType` — the type of a router a caller created.
///
/// The enum is `[custom, default]`: `default` names the routers Amazon Bedrock itself
/// publishes (which is why `ListPromptRouters$type` defaults to it and why the
/// `PromptRouterArn` pattern spells `default-prompt-router`), so a router that exists
/// because `CreatePromptRouter` was called is the other one.
pub const PROMPT_ROUTER_TYPE_CUSTOM: &str = "custom";

/// The granularity `RoutingCriteria$responseQualityDifference` is accepted at.
///
/// The member is modelled `@range(min: 0, max: 100)` and the boundary validator already
/// refuses a value outside that span; Amazon Bedrock additionally accepts only the
/// multiples of this step inside it, refusing anything else with 400
/// `ValidationException` (recorded in `simparity-behavioral-ga-prod-v1` as
/// `bedrock.create-prompt-router-invalid-routing-criteria-rejected`).
pub const RESPONSE_QUALITY_DIFFERENCE_STEP: f64 = 5.0;

/// The most prompt routers this account may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// The Amazon Bedrock service quota "Prompt routers per account" has a default of 500
/// (adjustable); this emulator enforces that default, so the store cannot grow without a
/// server-side bound no matter how many creates a caller issues.
pub const MAX_PROMPT_ROUTERS_PER_ACCOUNT: usize = 500;

/// The most tags one resource of this service may carry.
///
/// `TooManyTagsException` documents the bound itself — "The request contains more tags
/// than can be associated with a resource (50 tags per resource)" — so a create carrying
/// more is refused with that error rather than stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `MarketplaceModelEndpoint$status` — an endpoint Amazon Bedrock Marketplace knows.
///
/// The `Status` enum is `[REGISTERED, INCOMPATIBLE_ENDPOINT]`
/// (`smithy_query(query="shape",shape="Status")`) and the member documents it as "the
/// overall status of the endpoint in Amazon Bedrock Marketplace". An endpoint that exists
/// because `CreateMarketplaceModelEndpoint` was called was created by Bedrock from a
/// Marketplace model, so it is registered with Bedrock from that call onwards;
/// `INCOMPATIBLE_ENDPOINT` names an endpoint Bedrock cannot use, which no call here
/// produces.
pub const MARKETPLACE_ENDPOINT_REGISTERED: &str = "REGISTERED";

/// `MarketplaceModelEndpoint$endpointStatus` while the underlying endpoint is still being
/// brought up by the create call.
///
/// The member is a bare `String` documented as "the current status of the endpoint (e.g.,
/// Creating, InService, Updating, Failed)"
/// (`smithy_query(query="doc",shape="MarketplaceModelEndpoint",member="endpointStatus")`),
/// so these three constants are that documented spelling.
pub const ENDPOINT_STATUS_CREATING: &str = "Creating";

/// `MarketplaceModelEndpoint$endpointStatus` while a configuration change is being rolled
/// out by an update call. See [`ENDPOINT_STATUS_CREATING`] for the member's value list.
pub const ENDPOINT_STATUS_UPDATING: &str = "Updating";

/// `MarketplaceModelEndpoint$endpointStatus` once the endpoint is serving — the terminal
/// status the two non-terminal ones above settle into. See [`ENDPOINT_STATUS_CREATING`]
/// for the member's value list.
pub const ENDPOINT_STATUS_IN_SERVICE: &str = "InService";

/// How long after the call that starts it a marketplace endpoint's non-terminal
/// `endpointStatus` is owed its move to [`ENDPOINT_STATUS_IN_SERVICE`].
///
/// CHOSEN, not documented: AWS publishes no duration for bringing a Bedrock Marketplace
/// endpoint up, so this is the emulator's span. It is non-zero, so a caller that reads the
/// endpoint straight after creating or updating it still observes the transient status the
/// real service reports; and it is materialised from this stored due instant on the next
/// read rather than waited on, so no handler sleeps.
pub const ENDPOINT_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most Bedrock Marketplace model endpoints this account may hold at once, past which
/// a create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota (AWS documents none for this
/// resource): without a server-side cap a caller could grow this part of the store with
/// nothing but fresh endpoint names.
pub const MAX_MARKETPLACE_MODEL_ENDPOINTS_PER_ACCOUNT: usize = 100;

/// `ModelImportJobStatus` — the job is running.
///
/// The enum is `[InProgress, Completed, Failed]`
/// (`smithy_query(query="shape",shape="ModelImportJobStatus")`). A job Amazon Bedrock
/// accepted runs asynchronously, so this is the status it carries from the call that
/// starts it until the import is done.
pub const IMPORT_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ModelImportJobStatus` — the import finished, which is the terminal status the one
/// above is owed its move to.
///
/// `Failed` names a job whose import did not finish; nothing in this emulator makes an
/// accepted import fail, so no call here mints it.
pub const IMPORT_JOB_STATUS_COMPLETED: &str = "Completed";

/// How long after `CreateModelImportJob` a job's non-terminal `status` is owed its move
/// to [`IMPORT_JOB_STATUS_COMPLETED`].
///
/// CHOSEN, not documented: AWS publishes no duration for importing a customized model
/// (the real import takes minutes and depends on the model's size), so this is the
/// emulator's span. It is non-zero, so a caller that reads the job straight after
/// creating it still observes the `InProgress` status the real service reports; and it is
/// materialised from the stored due instant on the next read rather than waited on, so no
/// handler sleeps.
pub const IMPORT_JOB_COMPLETE_AFTER: Duration = Duration::from_millis(500);

/// The most model import jobs this account may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota (AWS publishes a quota for
/// CONCURRENT import jobs, not for the jobs an account's history holds): without a
/// server-side cap a caller could grow this part of the store with nothing but fresh job
/// names.
pub const MAX_MODEL_IMPORT_JOBS_PER_ACCOUNT: usize = 100;

/// `EvaluationJobStatus` — the job Amazon Bedrock accepted is running.
///
/// The enum is `[InProgress, Completed, Failed, Stopping, Stopped, Deleting]`
/// (`smithy_query(query="shape",shape="EvaluationJobStatus")`). An accepted evaluation job
/// runs asynchronously, so this is the status it carries from the call that starts it.
///
/// It is also the only status `CreateEvaluationJob` mints. Reaching `Completed` or
/// `Failed` means the inference and the metric computation over the caller's dataset ran
/// and the results were written to the request's `outputDataConfig` — work this emulator
/// does not perform, so no call here mints either, and no clock transition leads to them.
/// `Stopping` and `Stopped` are reached by a caller's own `StopEvaluationJob` (see
/// [`EVALUATION_JOB_STATUS_STOPPING`]) and `Deleting` by `BatchDeleteEvaluationJob`.
pub const EVALUATION_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `EvaluationJobStatus` — the job is winding down because a caller stopped it.
///
/// The status `StopEvaluationJob` moves a running job to: stopping a job is asynchronous,
/// so this is the non-terminal status the job carries from the call that asks for the stop
/// until the stop has taken effect. See [`EVALUATION_JOB_STATUS_IN_PROGRESS`] for the
/// value list.
pub const EVALUATION_JOB_STATUS_STOPPING: &str = "Stopping";

/// `EvaluationJobStatus` once the stop has taken effect — the terminal status the
/// non-terminal [`EVALUATION_JOB_STATUS_STOPPING`] is owed its move to, after
/// [`EVALUATION_JOB_STOPPED_AFTER`].
pub const EVALUATION_JOB_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopEvaluationJob` a job's `Stopping` status is owed its move to
/// [`EVALUATION_JOB_STATUS_STOPPED`].
///
/// CHOSEN, not documented: AWS publishes no duration for stopping an evaluation job (the
/// real stop has to wind down inference that is already in flight), so this is the
/// emulator's span. It is non-zero, so a caller that reads the job straight after stopping
/// it still observes the `Stopping` status the real service reports; and it is materialised
/// from the stored due instant on the next call that touches the record rather than waited
/// on, so no handler sleeps.
pub const EVALUATION_JOB_STOPPED_AFTER: Duration = Duration::from_millis(500);

/// `EvaluationJobType` — the job computes metrics itself.
///
/// The enum is `[Human, Automated]` (`smithy_query(query="shape",
/// shape="EvaluationJobType")`), and the `EvaluationConfig` union a create carries names
/// which one the job is: see [`evaluation_job_type_of`].
pub const EVALUATION_JOB_TYPE_AUTOMATED: &str = "Automated";

/// `EvaluationJobType` — the job's responses are rated by human workers. The other value
/// of the enum; see [`EVALUATION_JOB_TYPE_AUTOMATED`].
pub const EVALUATION_JOB_TYPE_HUMAN: &str = "Human";

/// The most evaluation jobs this account may hold at once, past which a create is refused
/// with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota (AWS publishes quotas for
/// CONCURRENT evaluation jobs per job type, not for the jobs an account's history holds):
/// without a server-side cap a caller could grow this part of the store with nothing but
/// fresh job names.
pub const MAX_EVALUATION_JOBS_PER_ACCOUNT: usize = 100;

/// `ProvisionedModelStatus` — the Provisioned Throughput is still being brought up by the
/// create call.
///
/// The enum is `[Creating, InService, Updating, Failed]`
/// (`smithy_query(query="shape",shape="ProvisionedModelStatus")`), which is also what
/// `crate::ops::types::PROVISIONED_MODEL_STATUS_VALUES` carries.
pub const PROVISIONED_MODEL_STATUS_CREATING: &str = "Creating";

/// `ProvisionedModelStatus` once the Provisioned Throughput is serving — the terminal
/// status the non-terminal one above is owed its move to. See
/// [`PROVISIONED_MODEL_STATUS_CREATING`] for the value list.
///
/// `Updating` names a Provisioned Throughput whose association is being changed and
/// `Failed` one that could not be provisioned; no call implemented here produces either,
/// so neither is minted.
pub const PROVISIONED_MODEL_STATUS_IN_SERVICE: &str = "InService";

/// How long after `CreateProvisionedModelThroughput` a Provisioned Throughput's
/// non-terminal `status` is owed its move to [`PROVISIONED_MODEL_STATUS_IN_SERVICE`].
///
/// CHOSEN, not documented: AWS publishes no duration for provisioning model units, so this
/// is the emulator's span. It is non-zero, so a caller that reads the Provisioned
/// Throughput straight after creating it still observes the `Creating` status the real
/// service reports; and it is materialised from the stored due instant on the next call
/// that touches the record rather than waited on, so no handler sleeps.
pub const PROVISIONED_MODEL_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most Provisioned Throughputs this account may hold at once in one region, past
/// which a create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota (AWS publishes per-model
/// model-unit quotas, not a count of Provisioned Throughputs an account may hold): without
/// a server-side cap a caller could grow this part of the store with nothing but fresh
/// names.
pub const MAX_PROVISIONED_MODEL_THROUGHPUTS_PER_ACCOUNT: usize = 100;

/// The model units an account has been granted for a Provisioned Throughput bought with
/// `commitment_duration`, past which a create is refused with
/// `ServiceQuotaExceededException`.
///
/// Amazon Bedrock grants model units by quota, and WHICH quota applies depends on whether
/// the Provisioned Throughput carries a commitment:
///
/// * without one, the no-commitment model-unit quota applies, and its default is ZERO —
///   an account has to have an increase granted before it can buy a Provisioned Throughput
///   without a commitment. RECORDED behaviour: the corpus `simparity-behavioral-ga-prod-v1`
///   records Amazon Bedrock refusing exactly such a create with
///   `ServiceQuotaExceededException`, so `modelUnits` (itself `range 1..`) always exceeds
///   what a fresh account has been granted there.
/// * with one, the commitment quota applies instead, which this emulator grants up to
///   [`MAX_COMMITTED_MODEL_UNITS`].
#[must_use]
pub fn granted_model_units(commitment_duration: Option<&str>) -> i32 {
    match commitment_duration {
        Some(_) => MAX_COMMITTED_MODEL_UNITS,
        None => 0,
    }
}

/// The most model units one Provisioned Throughput bought WITH a commitment may allocate.
///
/// CHOSEN, not published: AWS's per-model model-unit quotas differ per base model and are
/// adjustable, so this is the emulator's grant. It is what makes
/// [`granted_model_units`] a bound rather than an unbounded accept.
pub const MAX_COMMITTED_MODEL_UNITS: i32 = 64;

/// What `CreateProvisionedModelThroughput` answers as its `ServiceQuotaExceededException`
/// message for a Provisioned Throughput bought over `model_id` in `region` with no
/// commitment, which an account holds no granted model units for
/// ([`granted_model_units`]).
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus `simparity-behavioral-ga-prod-v1` records Amazon Bedrock naming the base model
/// the request asked for and the region it addressed, and pointing the caller at the
/// support case that raises the quota. So the rule is "name the model the request named,
/// in the region it addressed", not one recorded model id.
#[must_use]
pub fn no_commitment_model_units_message(model_id: &str, region: &str) -> String {
    format!(
        "Your account does not currently have any no commitment model units reserved for \
         {model_id}. Please see \
         https://support.console.aws.amazon.com/support/home?region={region}\
         #/case/create to request model units."
    )
}

/// `InferenceType` — the inference a Provisioned Throughput buys, as
/// `crate::ops::types::INFERENCE_TYPE_VALUES` (`[ON_DEMAND, PROVISIONED]`) spells it.
///
/// A base model is offered under one or both, which is what
/// [`FoundationModel::inference_types_supported`] reports, so it is also what decides
/// whether a Provisioned Throughput can be created over that model at all.
pub const INFERENCE_TYPE_PROVISIONED: &str = "PROVISIONED";

/// How long one month of a Provisioned Throughput commitment term lasts.
///
/// `CommitmentDuration` is `[OneMonth, SixMonths]`
/// (`smithy_query(query="shape",shape="CommitmentDuration")`), so the enum value itself
/// states the term in months and [`commitment_term`] multiplies this by the months it
/// names. AWS publishes no exact length for such a month, so 30 days is CHOSEN.
pub const COMMITMENT_MONTH: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The term a `CommitmentDuration` names, or `None` for a value that is not one of its
/// two.
///
/// The members this answers for are the whole enum, so a Provisioned Throughput created
/// with a commitment always has an expiration instant to report
/// ([`ProvisionedModelThroughputRecord::commitment_expiration_time`]).
#[must_use]
pub fn commitment_term(commitment_duration: &str) -> Option<Duration> {
    match commitment_duration {
        "OneMonth" => Some(COMMITMENT_MONTH),
        "SixMonths" => Some(6 * COMMITMENT_MONTH),
        _ => None,
    }
}

/// `ModelStatus` — a custom model that is still being validated and registered by the
/// call that created it.
///
/// The enum is `[Active, Creating, Failed]`
/// (`smithy_query(query="shape",shape="ModelStatus")`), and `CreateCustomModel` documents
/// exactly that progression for the model it creates: "`Creating` - Initial state during
/// validation and registration", "`Active` - Model is ready for use in inference",
/// "`Failed` - Creation process encountered an error"
/// (`smithy_query(query="doc",shape="CreateCustomModel")`).
pub const CUSTOM_MODEL_STATUS_CREATING: &str = "Creating";

/// `ModelStatus` once a custom model is ready for inference — the terminal status the
/// non-terminal one above is owed its move to. See [`CUSTOM_MODEL_STATUS_CREATING`] for
/// the value list.
///
/// `Failed` names a model whose creation did not finish; nothing here makes an accepted
/// create fail, so no call mints it.
pub const CUSTOM_MODEL_STATUS_ACTIVE: &str = "Active";

/// How long after `CreateCustomModel` a custom model's non-terminal `modelStatus` is owed
/// its move to [`CUSTOM_MODEL_STATUS_ACTIVE`].
///
/// CHOSEN, not documented: AWS publishes no duration for validating and registering model
/// artifacts, so this is the emulator's span. It is non-zero, so a caller that reads the
/// model straight after creating it still observes the `Creating` status the real service
/// reports; and it is materialised from the stored due instant on the next call that
/// touches the record rather than waited on, so no handler sleeps.
pub const CUSTOM_MODEL_ACTIVE_AFTER: Duration = Duration::from_millis(500);

/// `CustomizationType` — the customization a custom model created from model ARTIFACTS
/// carries.
///
/// The enum is `[FINE_TUNING, CONTINUED_PRE_TRAINING, DISTILLATION,
/// REINFORCEMENT_FINE_TUNING, IMPORTED]`
/// (`smithy_query(query="shape",shape="CustomizationType")`), and `CreateCustomModel`
/// documents that the model it creates "appears in `ListCustomModels` with a
/// `customizationType` of `imported`"
/// (`smithy_query(query="doc",shape="CreateCustomModel")`) — no training ran inside
/// Amazon Bedrock for it, the artifacts were brought in.
pub const CUSTOMIZATION_TYPE_IMPORTED: &str = "IMPORTED";

/// The base-model segment a `CustomModelArn` carries for a custom model whose artifacts
/// were brought in rather than customized from a foundation model.
///
/// `CustomModelArn` is
/// `…:custom-model/(imported|<provider>.<model>…)/[a-z0-9]{12}`
/// (`smithy_query(query="shape",shape="CustomModelSummary")`), so the ARN itself spells
/// the base of such a model with this literal token: the create request names no
/// foundation model, so none is invented for it.
pub const CUSTOM_MODEL_IMPORTED_BASE: &str = "imported";

/// The most custom models this account may hold at once, across every region it has
/// created one in, past which a create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota (AWS publishes quotas for
/// concurrent customization jobs, not for the custom models an account may hold): without
/// a server-side cap a caller could grow this part of the store with nothing but fresh
/// model names, or with fresh regions, which is why the bound is over the whole store
/// rather than per region.
pub const MAX_CUSTOM_MODELS_PER_ACCOUNT: usize = 100;

/// The number of base-36 digits each half of a minted resource id carries.
///
/// `GuardrailId` is `^[a-z0-9]+$` with length at most 64; base 36 over
/// `0-9a-z` is exactly that alphabet.
const ID_HALF_WIDTH: usize = 6;

/// One immutable-or-working configuration of a guardrail: the DRAFT, or a
/// numbered version snapshotted from it.
#[derive(Debug, Clone)]
pub struct GuardrailRevision {
    /// `DRAFT`, or the decimal version number (`^[1-9][0-9]{0,7}$`).
    pub version: String,
    /// This revision's own description (the DRAFT's, or the one passed to
    /// `CreateGuardrailVersion`).
    pub description: Option<String>,
    pub blocked_input_messaging: String,
    pub blocked_outputs_messaging: String,
    pub topic_policy: Option<GuardrailTopicPolicy>,
    pub content_policy: Option<GuardrailContentPolicy>,
    pub word_policy: Option<GuardrailWordPolicy>,
    pub sensitive_information_policy: Option<GuardrailSensitiveInformationPolicy>,
    pub contextual_grounding_policy: Option<GuardrailContextualGroundingPolicy>,
    pub automated_reasoning_policy: Option<GuardrailAutomatedReasoningPolicy>,
    pub cross_region_details: Option<GuardrailCrossRegionDetails>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// This revision's `GuardrailStatus`. [`STATUS_READY`] for every revision this
    /// emulator mints; see that constant for why no other value is reachable.
    pub status: &'static str,
    /// `statusReasons` — why the guardrail failed to be created, updated, versioned or
    /// deleted. Empty for a revision that is not `FAILED`, which is every revision here,
    /// and reported as the empty list rather than omitted.
    pub status_reasons: Vec<String>,
    /// `failureRecommendations` — what to do before retrying. Empty alongside
    /// [`GuardrailRevision::status_reasons`], and reported the same way.
    pub failure_recommendations: Vec<String>,
}

impl GuardrailRevision {
    /// A revision carrying only the two required messaging strings, created at `now`.
    pub fn new(
        version: impl Into<String>,
        blocked_input_messaging: String,
        blocked_outputs_messaging: String,
        now: SystemTime,
    ) -> Self {
        Self {
            version: version.into(),
            description: None,
            blocked_input_messaging,
            blocked_outputs_messaging,
            topic_policy: None,
            content_policy: None,
            word_policy: None,
            sensitive_information_policy: None,
            contextual_grounding_policy: None,
            automated_reasoning_policy: None,
            cross_region_details: None,
            created_at: now,
            updated_at: now,
            status: STATUS_READY,
            status_reasons: Vec::new(),
            failure_recommendations: Vec::new(),
        }
    }

    /// Replace this revision's whole policy set with `configs`, converted to the
    /// response shapes. A config the request omits CLEARS that policy: a
    /// guardrail's policy set is replaced, never merged.
    pub fn set_policies(&mut self, configs: GuardrailPolicyConfigs, region: &str) {
        self.topic_policy = configs.topic.as_ref().map(topic_policy_of);
        self.content_policy = configs.content.as_ref().map(content_policy_of);
        self.word_policy = configs.word.as_ref().map(word_policy_of);
        self.sensitive_information_policy = configs
            .sensitive_information
            .as_ref()
            .map(sensitive_information_policy_of);
        self.contextual_grounding_policy = configs
            .contextual_grounding
            .as_ref()
            .map(contextual_grounding_policy_of);
        self.automated_reasoning_policy = configs
            .automated_reasoning
            .as_ref()
            .map(automated_reasoning_policy_of);
        self.cross_region_details = configs
            .cross_region
            .as_ref()
            .and_then(|config| cross_region_details_of(config, region));
    }
}

/// The policy configuration a `CreateGuardrail` or `UpdateGuardrail` request
/// carries, lifted out of those two request shapes so one conversion serves both.
#[derive(Debug, Default)]
pub struct GuardrailPolicyConfigs {
    pub topic: Option<GuardrailTopicPolicyConfig>,
    pub content: Option<GuardrailContentPolicyConfig>,
    pub word: Option<GuardrailWordPolicyConfig>,
    pub sensitive_information: Option<GuardrailSensitiveInformationPolicyConfig>,
    pub contextual_grounding: Option<GuardrailContextualGroundingPolicyConfig>,
    pub automated_reasoning: Option<GuardrailAutomatedReasoningPolicyConfig>,
    pub cross_region: Option<GuardrailCrossRegionConfig>,
}

/// A stored guardrail: the identity that never changes, plus its DRAFT and every
/// numbered version snapshotted from it.
#[derive(Debug, Clone)]
pub struct Guardrail {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub kms_key_arn: Option<String>,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub draft: GuardrailRevision,
    pub versions: BTreeMap<u32, GuardrailRevision>,
    /// `CreateGuardrailVersion` idempotency: `clientRequestToken` -> the version
    /// number that token cut.
    ///
    /// It lives INSIDE the record, and an entry is dropped when the version it
    /// names is deleted, so the map holds at most one entry per live version
    /// (bounded by [`MAX_VERSIONS_PER_GUARDRAIL`]) and dies with the guardrail by
    /// construction.
    pub version_tokens: HashMap<String, u32>,
    /// The number `CreateGuardrailVersion` will mint next, from `1` upwards.
    pub next_version: u32,
}

impl Guardrail {
    /// The revision a `guardrailVersion` names. An absent version, and the literal
    /// `DRAFT`, both name the DRAFT; anything else must be a stored number.
    pub fn revision(&self, version: Option<&str>) -> Option<&GuardrailRevision> {
        match version {
            None => Some(&self.draft),
            Some(DRAFT_VERSION) => Some(&self.draft),
            Some(number) => version_number(number).and_then(|n| self.versions.get(&n)),
        }
    }

    /// Every revision of this guardrail in list order: the DRAFT, then the
    /// numbered versions ascending.
    pub fn revisions(&self) -> impl Iterator<Item = &GuardrailRevision> {
        std::iter::once(&self.draft).chain(self.versions.values())
    }

    /// Remove version `number` and release the idempotency tokens that named it, so
    /// a token never outlives the version it minted.
    pub fn remove_version(&mut self, number: u32) -> Option<GuardrailRevision> {
        let removed = self.versions.remove(&number)?;
        self.version_tokens.retain(|_, minted| *minted != number);
        Some(removed)
    }
}

/// One stored prompt router: the identity `CreatePromptRouter` minted, the
/// configuration that request carried, and the two instants a read reports.
///
/// `status` and `router_type` are `&'static str` for the same reason
/// [`GuardrailRevision::status`] is: both are single values this emulator mints rather
/// than anything a request chooses ([`PROMPT_ROUTER_AVAILABLE`],
/// [`PROMPT_ROUTER_TYPE_CUSTOM`]).
#[derive(Debug, Clone)]
pub struct PromptRouter {
    /// The `PromptRouterArn` this router is addressed by, minted by
    /// [`prompt_router_arn`].
    pub arn: String,
    /// `promptRouterName`, as the create request spelled it.
    pub name: String,
    pub description: Option<String>,
    /// `models` — the foundation models this router may route to, in request order.
    pub models: Vec<PromptRouterTargetModel>,
    pub fallback_model: PromptRouterTargetModel,
    pub routing_criteria: RoutingCriteria,
    pub tags: Vec<Tag>,
    /// `PromptRouterStatus`; [`PROMPT_ROUTER_AVAILABLE`] for every router minted here.
    pub status: &'static str,
    /// `PromptRouterType`; [`PROMPT_ROUTER_TYPE_CUSTOM`] for a router a caller created.
    pub router_type: &'static str,
    pub created_at: SystemTime,
    /// When the router last changed. Equal to [`PromptRouter::created_at`] until some
    /// operation changes the router; the model carries no `UpdatePromptRouter`, so no
    /// request moves it today.
    pub updated_at: SystemTime,
}

/// One stored Bedrock Marketplace model endpoint: the identity
/// `CreateMarketplaceModelEndpoint` minted, the configuration the latest call carried, the
/// two instants a read reports, and the endpoint lifecycle status.
///
/// `status` is `&'static str` for the same reason [`PromptRouter::status`] is: it is the
/// single value this emulator mints ([`MARKETPLACE_ENDPOINT_REGISTERED`]) rather than
/// anything a request chooses.
#[derive(Debug, Clone)]
pub struct MarketplaceModelEndpointRecord {
    /// The `endpointArn` this endpoint is addressed by, minted by
    /// [`marketplace_model_endpoint_arn`].
    pub arn: String,
    /// `endpointName`, as the create request spelled it. The ARN's resource id, so it is
    /// also what makes the name unique within the account and region.
    pub name: String,
    /// `modelSourceIdentifier` — the Marketplace model this endpoint serves.
    pub model_source_identifier: String,
    /// `endpointConfig` — the create request's configuration, REPLACED whole by each
    /// `UpdateMarketplaceModelEndpoint`.
    pub endpoint_config: EndpointConfig,
    /// `acceptEula`, as the create request left it (the member defaults to `false`).
    ///
    /// Kept with the record because it is part of the request that minted the endpoint; no
    /// response member of this service reports it, so nothing reads it back today.
    pub accept_eula: bool,
    pub tags: Vec<Tag>,
    /// `MarketplaceModelEndpoint$status`; [`MARKETPLACE_ENDPOINT_REGISTERED`] for every
    /// endpoint minted here.
    pub status: &'static str,
    /// `MarketplaceModelEndpoint$endpointStatus` — the endpoint's own lifecycle status.
    pub endpoint_status: &'static str,
    /// When [`Self::endpoint_status`] is owed its move to
    /// [`ENDPOINT_STATUS_IN_SERVICE`], or `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer: [`Self::settle`] materialises the move on the next
    /// call that touches the endpoint, so the transition is owed at the same instant
    /// whether or not anybody calls, and no handler sleeps for it.
    pub in_service_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the endpoint's configuration last changed: the creation instant until an
    /// `UpdateMarketplaceModelEndpoint` moves it.
    pub updated_at: SystemTime,
    /// The `clientRequestToken` of the most recent `UpdateMarketplaceModelEndpoint`
    /// applied to this endpoint, so a RETRY of that same call — the request the
    /// `@idempotencyToken` member is there for — applies nothing a second time.
    ///
    /// ONE token per endpoint rather than a map of every token ever seen, so this cannot
    /// grow with the number of updates a caller issues.
    pub last_update_token: Option<String>,
}

impl MarketplaceModelEndpointRecord {
    /// Start a non-terminal `endpointStatus`, due to settle
    /// [`ENDPOINT_SETTLE_AFTER`] from `now`.
    pub fn begin(&mut self, status: &'static str, now: SystemTime) {
        self.endpoint_status = status;
        self.in_service_due_at = Some(now + ENDPOINT_SETTLE_AFTER);
    }

    /// Materialise the move to [`ENDPOINT_STATUS_IN_SERVICE`] the clock has already made
    /// due at `now`, and report the status to report.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at `now`
    /// even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.in_service_due_at.is_some_and(|due| now >= due) {
            self.endpoint_status = ENDPOINT_STATUS_IN_SERVICE;
            self.in_service_due_at = None;
        }
    }
}

/// One stored model import job: the identity `CreateModelImportJob` minted, the request
/// that minted it, the instants a read reports, and the job's own lifecycle status.
///
/// `status` is `&'static str` for the same reason [`PromptRouter::status`] is: this
/// emulator mints only the two values the lifecycle passes through
/// ([`IMPORT_JOB_STATUS_IN_PROGRESS`], [`IMPORT_JOB_STATUS_COMPLETED`]) rather than
/// anything a request chooses.
#[derive(Debug, Clone)]
pub struct ModelImportJobRecord {
    /// The `ModelImportJobArn` this job is addressed by, minted by
    /// [`model_import_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it — the other spelling a
    /// `ModelImportJobIdentifier` may carry.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index and the uniqueness checks are per region without re-parsing an
    /// ARN.
    pub region: String,
    /// `importedModelName` — the name of the model this job imports.
    pub imported_model_name: String,
    /// The `ImportedModelArn` of the model this job imports, minted by
    /// [`imported_model_arn`] when the job is accepted: it is the job's target, so it is
    /// fixed from that call onwards.
    pub imported_model_arn: String,
    /// `roleArn` — the service role Amazon Bedrock assumes for the import.
    pub role_arn: String,
    /// `modelDataSource` — where the model being imported is read from.
    pub model_data_source: ModelDataSource,
    /// `vpcConfig`, as the create request carried it (absent when it carried none).
    pub vpc_config: Option<VpcConfig>,
    /// The `importedModelKmsKeyArn` a read reports for the request's
    /// `importedModelKmsKeyId`, as [`kms_key_arn`] completes it.
    pub imported_model_kms_key_arn: Option<String>,
    /// `jobTags` — the tags attached to the import job itself.
    pub job_tags: Vec<Tag>,
    /// `importedModelTags` — the tags attached to the model the job imports.
    pub imported_model_tags: Vec<Tag>,
    /// `ModelImportJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`IMPORT_JOB_STATUS_COMPLETED`], or
    /// `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed: the creation instant until the import completes.
    pub last_modified_at: SystemTime,
    /// When the job ended, or `None` while it is still running.
    pub ended_at: Option<SystemTime>,
}

impl ModelImportJobRecord {
    /// Materialise the move to [`IMPORT_JOB_STATUS_COMPLETED`] the clock has already made
    /// due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its end.
    pub fn complete(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = IMPORT_JOB_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.ended_at = Some(due);
            self.last_modified_at = due;
        }
    }
}

/// One stored evaluation job: the identity `CreateEvaluationJob` minted, the
/// configuration that request carried, and the instants a read reports.
///
/// `status` and `job_type` are `&'static str` for the same reason
/// [`ModelImportJobRecord::status`] is: both are values this emulator derives
/// ([`EVALUATION_JOB_STATUS_IN_PROGRESS`], [`evaluation_job_type_of`]) rather than
/// anything a request names directly.
#[derive(Debug, Clone)]
pub struct EvaluationJobRecord {
    /// The `EvaluationJobArn` this job is addressed by, minted by [`evaluation_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN.
    pub region: String,
    /// `jobDescription`, as the create request carried it (absent when it carried none).
    pub description: Option<String>,
    /// `roleArn` — the service role Amazon Bedrock assumes to run the evaluation.
    pub role_arn: String,
    /// `customerEncryptionKeyId`, as the create request carried it. Stored and reported
    /// VERBATIM: the read's `customerEncryptionKeyId` carries the same `KmsKeyId` shape
    /// the request's does, so a key ARN, a bare key id and an alias all come back as they
    /// were given (unlike a member a read reports as a `KmsKeyArn`, which must be
    /// completed by [`kms_key_arn`]).
    pub customer_encryption_key_id: Option<String>,
    /// `applicationType`, as the create request carried it (absent when it carried none —
    /// the member is optional in both the request and the response, and the model declares
    /// no default for it).
    pub application_type: Option<String>,
    /// `evaluationConfig` — the automated or human evaluation the job runs.
    pub evaluation_config: EvaluationConfig,
    /// `inferenceConfig` — the models or knowledge bases the job evaluates.
    pub inference_config: EvaluationInferenceConfig,
    /// `outputDataConfig` — where the job's results are saved.
    pub output_data_config: EvaluationOutputDataConfig,
    /// `jobTags` — the tags attached to the evaluation job.
    pub tags: Vec<Tag>,
    /// `EvaluationJobType`, as [`evaluation_job_type_of`] reads it off
    /// [`Self::evaluation_config`].
    pub job_type: &'static str,
    /// `EvaluationJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`EVALUATION_JOB_STATUS_STOPPED`], or
    /// `None` while no stop is in flight.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub stopped_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed, which is the creation instant until something changes
    /// it.
    pub last_modified_at: SystemTime,
}

impl EvaluationJobRecord {
    /// Whether this job is one a stop may be asked for: it is still running.
    ///
    /// "Stops an evaluation job that is current being created or running"
    /// (`smithy_query(query="operation",operation="StopEvaluationJob")`), so a job in any
    /// other status has nothing left to stop.
    #[must_use]
    pub fn is_stoppable(&self) -> bool {
        self.status == EVALUATION_JOB_STATUS_IN_PROGRESS
    }

    /// Move this job to [`EVALUATION_JOB_STATUS_STOPPING`] and record when the stop is owed
    /// its move to [`EVALUATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopEvaluationJob` — not a clock transition:
    /// [`Self::settle`] is what materialises the one the clock owes.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = EVALUATION_JOB_STATUS_STOPPING;
        self.stopped_due_at = Some(now + EVALUATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
    }

    /// Materialise the move to [`EVALUATION_JOB_STATUS_STOPPED`] the clock has already made
    /// due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its last modification.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = EVALUATION_JOB_STATUS_STOPPED;
            self.stopped_due_at = None;
            self.last_modified_at = due;
        }
    }
}

/// One stored Provisioned Throughput: the identity `CreateProvisionedModelThroughput`
/// minted, the model units and commitment that request bought, the model it is associated
/// with, and the lifecycle status a read reports.
///
/// `status` is `&'static str` for the same reason [`ModelImportJobRecord::status`] is: it
/// is one of the two values this emulator mints
/// ([`PROVISIONED_MODEL_STATUS_CREATING`], [`PROVISIONED_MODEL_STATUS_IN_SERVICE`])
/// rather than anything a request chooses.
#[derive(Debug, Clone)]
pub struct ProvisionedModelThroughputRecord {
    /// The `ProvisionedModelArn` this Provisioned Throughput is addressed by, minted by
    /// [`provisioned_model_arn`].
    pub arn: String,
    /// `provisionedModelName`, as the create request spelled it. The other spelling of
    /// `ProvisionedModelId`, so it is also what makes the name unique within the account
    /// and region.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN, exactly as
    /// [`EvaluationJobRecord::region`] is.
    pub region: String,
    /// `modelArn` — the model this Provisioned Throughput is associated with NOW.
    pub model_arn: String,
    /// `desiredModelArn` — the model it was last requested to be associated with. Equal to
    /// [`Self::model_arn`] until an association change is in flight.
    pub desired_model_arn: String,
    /// `foundationModelArn` — the base model the Provisioned Throughput was created for,
    /// as [`foundation_model_arn`] spells it.
    pub foundation_model_arn: String,
    /// `modelUnits` — the model units allocated now.
    pub model_units: i32,
    /// `desiredModelUnits` — the model units last requested. Equal to [`Self::model_units`]
    /// until an allocation change is in flight.
    pub desired_model_units: i32,
    /// `ProvisionedModelStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`PROVISIONED_MODEL_STATUS_IN_SERVICE`],
    /// or `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub in_service_due_at: Option<SystemTime>,
    /// `commitmentDuration`, as the create request carried it (absent for a no-commitment
    /// Provisioned Throughput).
    pub commitment_duration: Option<String>,
    /// `commitmentExpirationTime` — when the commitment term ends: the creation instant
    /// plus the term [`commitment_term`] reads off [`Self::commitment_duration`], and
    /// `None` when there is no commitment.
    pub commitment_expiration_time: Option<SystemTime>,
    /// `tags` — the tags the create request attached.
    ///
    /// Kept with the record because it is part of the request that minted it; the
    /// Provisioned Throughput read operations report no tag member, so nothing reads them
    /// back until `ListTagsForResource` is implemented.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// When the Provisioned Throughput last changed: the creation instant until something
    /// changes it, and the instant a settle was owed once the status has moved.
    pub last_modified_at: SystemTime,
}

impl ProvisionedModelThroughputRecord {
    /// Materialise the move to [`PROVISIONED_MODEL_STATUS_IN_SERVICE`] the clock has
    /// already made due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the record reports as its last modification.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.in_service_due_at.filter(|due| now >= *due) {
            self.status = PROVISIONED_MODEL_STATUS_IN_SERVICE;
            self.in_service_due_at = None;
            self.last_modified_at = due;
        }
    }
}

/// One stored custom model: the identity `CreateCustomModel` minted, the data source and
/// key that request carried, the base model it reports, and the status a read reports.
///
/// `status` and `customization_type` are `&'static str` for the same reason
/// [`ModelImportJobRecord::status`] is: both are values this emulator derives
/// ([`CUSTOM_MODEL_STATUS_CREATING`], [`CUSTOMIZATION_TYPE_IMPORTED`]) rather than
/// anything a request names.
#[derive(Debug, Clone)]
pub struct CustomModelRecord {
    /// The `CustomModelArn` this model is addressed by, minted by [`custom_model_arn`].
    pub arn: String,
    /// `modelName`, as the create request spelled it \u2014 the other spelling a
    /// `ModelIdentifier` may carry.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN, exactly as
    /// [`ProvisionedModelThroughputRecord::region`] is.
    pub region: String,
    /// `baseModelArn` \u2014 the model this one was customized from.
    ///
    /// For a model created from ARTIFACTS this is the model's OWN ARN: the create request
    /// names no foundation model, and `ModelArn` admits a `custom-model` ARN here, so the
    /// model is reported as its own base rather than attributed to a foundation model
    /// nothing in the request named.
    pub base_model_arn: String,
    /// `baseModelName` \u2014 the base model's name, which for a model created from artifacts
    /// is the base segment its own ARN carries ([`CUSTOM_MODEL_IMPORTED_BASE`]).
    pub base_model_name: String,
    /// The `FoundationModelArn` of the foundation model this custom model descends from,
    /// or `None` when it descends from none \u2014 which is what `ListCustomModels`'
    /// `foundationModelArnEquals` filters on, as distinct from `baseModelArnEquals`.
    pub foundation_model_arn: Option<String>,
    /// `CustomizationType`.
    pub customization_type: &'static str,
    /// `modelSourceConfig`, as the create request carried it (absent when the request
    /// named the model package source instead).
    pub model_source_config: Option<ModelDataSource>,
    /// `customModelDataSource`, as the create request carried it (absent when the request
    /// named the Amazon S3 source instead).
    pub custom_model_data_source: Option<CustomModelDataSource>,
    /// `modelKmsKeyArn`, as the create request carried it \u2014 already a `KmsKeyArn` there,
    /// so it is stored and reported verbatim.
    pub model_kms_key_arn: Option<String>,
    /// `roleArn` \u2014 the service role Amazon Bedrock assumes to read the artifacts (absent
    /// when the create carried none).
    ///
    /// Kept with the record because it is part of the request that minted it; no read
    /// operation of this resource reports a role member.
    pub role_arn: Option<String>,
    /// `modelTags` \u2014 the tags the create request attached to the model.
    ///
    /// Kept for the reason [`ProvisionedModelThroughputRecord::tags`] is: it is part of
    /// the request that minted the record, and nothing reads it back until
    /// `ListTagsForResource` is implemented.
    pub tags: Vec<Tag>,
    /// `ModelStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`CUSTOM_MODEL_STATUS_ACTIVE`], or `None`
    /// once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub active_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
}

impl CustomModelRecord {
    /// Materialise the move to [`CUSTOM_MODEL_STATUS_ACTIVE`] the clock has already made
    /// due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.active_due_at.is_some_and(|due| now >= due) {
            self.status = CUSTOM_MODEL_STATUS_ACTIVE;
            self.active_due_at = None;
        }
    }
}

/// The account's enforced guardrail configuration: the guardrail revision every
/// invocation in this account is guarded by, as `PutEnforcedGuardrailConfiguration` last
/// installed it.
///
/// ONE record per account rather than a map, because the listing that reports it is
/// modelled `@length(min: 0, max: 1)`
/// (`smithy_query(query="operation",operation="ListEnforcedGuardrailsConfiguration")`):
/// an account holds at most one such configuration, so a further put REPLACES this record
/// instead of adding another.
///
/// The guardrail is stored as the id and the ARN of the guardrail the request's
/// `guardrailIdentifier` resolved to, rather than as the identifier's own spelling, so the
/// listing reports the same resource whichever spelling installed it.
#[derive(Debug, Clone)]
pub struct EnforcedGuardrailConfiguration {
    /// `configId` — the id this configuration is addressed by, minted by
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::mint_enforced_guardrail_config_id`]
    /// when the first put installed it.
    pub config_id: String,
    /// `guardrailId` of the guardrail this configuration enforces.
    pub guardrail_id: String,
    /// `guardrailArn` of that same guardrail, as the stored guardrail carries it.
    pub guardrail_arn: String,
    /// `guardrailVersion` — the numbered version being enforced
    /// (`GuardrailNumericalVersion`, so never the DRAFT).
    pub guardrail_version: String,
    /// `selectiveContentGuarding`, as the put carried it (absent when it carried none).
    pub selective_content_guarding: Option<SelectiveContentGuarding>,
    /// `modelEnforcement`, as the put carried it (absent when it carried none).
    pub model_enforcement: Option<ModelEnforcement>,
    /// When the configuration was first installed; kept across a later put, which
    /// replaces the configuration rather than creating a new one.
    pub created_at: SystemTime,
    /// The principal that first installed it ([`caller_principal`]).
    pub created_by: String,
    /// When the last admitted put stored it.
    pub updated_at: SystemTime,
    /// The principal that made that put ([`caller_principal`]).
    pub updated_by: String,
}

/// Per-service state for com-amazonaws-bedrock-amazonbedrockcontrolplaneservice. A handler reaches it through [`service_state`].
///
/// Every map here is bounded, and each bound holds by construction rather than by
/// convention:
///
/// * `guardrails` and `guardrail_names` hold one entry per live guardrail, capped at
///   [`MAX_GUARDRAILS_PER_ACCOUNT`] by the quota `CreateGuardrail` enforces.
/// * each guardrail's `versions` holds one entry per live version, capped at
///   [`MAX_VERSIONS_PER_GUARDRAIL`] by the quota `CreateGuardrailVersion` enforces.
/// * `create_tokens` records an entry only for a create that was ADMITTED (a refused
///   create records nothing), every entry names a live guardrail, and the entries
///   naming a guardrail are dropped when it is deleted — so it holds at most one
///   entry per live guardrail.
/// * each guardrail's [`Guardrail::version_tokens`] is bounded the same way against
///   its live versions, including when a SINGLE version is deleted
///   ([`Guardrail::remove_version`]).
///
/// * `prompt_routers` and `prompt_router_names` hold one entry per live prompt router,
///   capped at [`MAX_PROMPT_ROUTERS_PER_ACCOUNT`] by the quota `CreatePromptRouter`
///   enforces, and `prompt_router_create_tokens` holds at most one entry per live router
///   for the same reason `create_tokens` does.
///
/// * `model_invocation_logging` is a single optional settings record, which a put
///   replaces rather than appends to.
///
/// * `model_import_jobs` and `model_import_job_names` hold one entry per stored model
///   import job, capped at [`MAX_MODEL_IMPORT_JOBS_PER_ACCOUNT`] by the quota
///   `CreateModelImportJob` enforces, and `model_import_job_create_tokens` holds at most
///   one entry per stored job for the same reason `create_tokens` does.
///
/// * `evaluation_jobs` and `evaluation_job_names` hold one entry per stored evaluation
///   job, capped at [`MAX_EVALUATION_JOBS_PER_ACCOUNT`] by the quota
///   `CreateEvaluationJob` enforces, and `evaluation_job_create_tokens` holds at most one
///   entry per stored job for the same reason `create_tokens` does.
///
/// * `marketplace_model_endpoints` holds one entry per live marketplace model endpoint,
///   capped at [`MAX_MARKETPLACE_MODEL_ENDPOINTS_PER_ACCOUNT`] by the quota
///   `CreateMarketplaceModelEndpoint` enforces (the entry is keyed by the endpoint ARN,
///   which carries the endpoint name, so two creates of one name in one region cannot add
///   two entries), `marketplace_endpoint_create_tokens` holds at most one entry per live
///   endpoint for the same reason `create_tokens` does, and each endpoint's
///   [`MarketplaceModelEndpointRecord::last_update_token`] is ONE token rather than a map,
///   so repeated updates cannot grow the store either.
///
/// * `provisioned_model_throughputs` and `provisioned_model_names` hold one entry per
///   live Provisioned Throughput, capped at
///   [`MAX_PROVISIONED_MODEL_THROUGHPUTS_PER_ACCOUNT`] by the quota
///   `CreateProvisionedModelThroughput` enforces, and `provisioned_model_create_tokens`
///   holds at most one entry per live Provisioned Throughput for the same reason
///   `create_tokens` does — including across a delete, which drops the entries naming the
///   Provisioned Throughput it removed.
///
/// * `custom_models` and `custom_model_names` hold one entry per stored custom model,
///   capped at [`MAX_CUSTOM_MODELS_PER_ACCOUNT`] by the quota `CreateCustomModel`
///   enforces, and `custom_model_create_tokens` holds at most one entry per stored custom
///   model for the same reason `create_tokens` does.
///
/// * `foundation_models` is the PUBLISHED catalog, seeded once per store and never
///   written: no operation here creates or deletes a foundation model.
///
/// * `use_case_for_model_access` is a single optional form, which a put replaces rather
///   than appends to, and whose size the model already caps
///   (`AcknowledgementFormDataBody` is 10..16384 bytes).
///
/// So the whole store is bounded by the live resources the quotas admit, and no
/// sequence of requests — including create/delete cycles with fresh idempotency
/// tokens — can grow it past them.
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState {
    /// Every stored guardrail, keyed by its `guardrailId`.
    pub guardrails: HashMap<String, Guardrail>,
    /// `name` -> `guardrailId`, so a duplicate name is a `ConflictException`.
    pub guardrail_names: HashMap<String, String>,
    /// `CreateGuardrail` idempotency: `clientRequestToken` -> the `guardrailId`
    /// that token minted. Scoped to that one operation, so a token replayed
    /// against another operation is an independent request.
    pub create_tokens: HashMap<String, String>,
    /// How many guardrail ids this store has minted; the uniqueness half of a
    /// minted id.
    pub minted_guardrails: u64,
    /// The model invocation logging configuration currently in force, as
    /// `PutModelInvocationLoggingConfiguration` last stored it, or `None` while none
    /// is configured (nothing was ever put, or
    /// `DeleteModelInvocationLoggingConfiguration` removed it).
    ///
    /// ONE record, not a map: model invocation logging is a single account-level
    /// settings record rather than a resource a caller can create more than one of,
    /// so a put REPLACES this field whole and the store cannot grow by putting
    /// again. It is keyed by nothing for the same reason the guardrail store is not
    /// keyed by region: this store models the one account the emulator serves.
    pub model_invocation_logging: Option<LoggingConfig>,
    /// The account's enforced guardrail configuration, as
    /// `PutEnforcedGuardrailConfiguration` last installed it, or `None` while the account
    /// has none.
    ///
    /// ONE record for the reason [`EnforcedGuardrailConfiguration`] gives: the listing that
    /// reports it admits at most one entry, so a put replaces this field whole and the
    /// store cannot grow by putting again.
    pub enforced_guardrail_configuration: Option<EnforcedGuardrailConfiguration>,
    /// How many enforced guardrail configuration ids this store has minted; the uniqueness
    /// half of a minted id.
    pub minted_enforced_guardrail_configs: u64,
    /// Every stored prompt router, keyed by its `PromptRouterArn` — the one identifier
    /// `GetPromptRouter` and `DeletePromptRouter` address a router by.
    pub prompt_routers: HashMap<String, PromptRouter>,
    /// `(region, promptRouterName)` -> `PromptRouterArn`, so a duplicate name is a
    /// `ConflictException`.
    ///
    /// Keyed by the region too because the model documents the name as unique "within
    /// your Amazon Web Services account in the current region"
    /// (`smithy_query(query="doc",shape="CreatePromptRouterRequest",member="promptRouterName")`),
    /// so the same name in another region names another router.
    pub prompt_router_names: HashMap<(String, String), String>,
    /// `CreatePromptRouter` idempotency: `(region, clientRequestToken)` -> the
    /// `PromptRouterArn` that token minted. Scoped to that one operation, and to the
    /// region the request addressed, the way the name index is.
    pub prompt_router_create_tokens: HashMap<(String, String), String>,
    /// How many prompt router ids this store has minted; the uniqueness half of a
    /// minted id.
    pub minted_prompt_routers: u64,
    /// Every stored Bedrock Marketplace model endpoint, keyed by its `endpointArn` — the
    /// one identifier `GetMarketplaceModelEndpoint`, `UpdateMarketplaceModelEndpoint` and
    /// `DeleteMarketplaceModelEndpoint` address an endpoint by.
    ///
    /// The ARN carries both the region and the endpoint name
    /// ([`marketplace_model_endpoint_arn`]), so this one map is also the per-region name
    /// index: the same name in another region is another key.
    pub marketplace_model_endpoints: HashMap<String, MarketplaceModelEndpointRecord>,
    /// `CreateMarketplaceModelEndpoint` idempotency: `(region, clientRequestToken)` -> the
    /// `endpointArn` that token minted. Scoped to that one operation, and to the region
    /// the request addressed, the way the prompt router index is.
    pub marketplace_endpoint_create_tokens: HashMap<(String, String), String>,
    /// Every stored model import job, keyed by its `ModelImportJobArn` — one of the two
    /// spellings `GetModelImportJob` addresses a job by.
    pub model_import_jobs: HashMap<String, ModelImportJobRecord>,
    /// `(region, jobName)` -> `ModelImportJobArn`, so a duplicate job name is a
    /// `ConflictException` and the other `ModelImportJobIdentifier` spelling — the bare
    /// job name — resolves to the job it names.
    pub model_import_job_names: HashMap<(String, String), String>,
    /// `CreateModelImportJob` idempotency: `(region, clientRequestToken)` -> the `jobArn`
    /// that token minted. Scoped to that one operation, and to the region the request
    /// addressed, the way the prompt router index is.
    pub model_import_job_create_tokens: HashMap<(String, String), String>,
    /// How many model import job ids this store has minted; the uniqueness half of a
    /// minted id.
    pub minted_model_import_jobs: u64,
    /// How many imported model ids this store has minted; the uniqueness half of a minted
    /// id.
    pub minted_imported_models: u64,
    /// Every stored evaluation job, keyed by its `EvaluationJobArn` — the one spelling an
    /// `EvaluationJobIdentifier` carries
    /// (`smithy_query(query="operation",operation="GetEvaluationJob")`: its pattern is the
    /// job ARN alone).
    pub evaluation_jobs: HashMap<String, EvaluationJobRecord>,
    /// `(region, jobName)` -> `EvaluationJobArn`, so a duplicate job name is a
    /// `ConflictException`.
    ///
    /// Keyed by the region too because the model documents the name as unique "with your
    /// Amazon Web Services account, and your account's Amazon Web Services region"
    /// (`smithy_query(query="doc",shape="CreateEvaluationJobRequest",member="jobName")`),
    /// so the same name in another region names another job.
    pub evaluation_job_names: HashMap<(String, String), String>,
    /// `CreateEvaluationJob` idempotency: `(region, clientRequestToken)` -> the `jobArn`
    /// that token minted. Scoped to that one operation, and to the region the request
    /// addressed, the way the model import job index is.
    pub evaluation_job_create_tokens: HashMap<(String, String), String>,
    /// How many evaluation job ids this store has minted; the uniqueness half of a minted
    /// id.
    pub minted_evaluation_jobs: u64,
    /// Every stored Provisioned Throughput, keyed by its `ProvisionedModelArn` — one of
    /// the two spellings `ProvisionedModelId` admits.
    pub provisioned_model_throughputs: HashMap<String, ProvisionedModelThroughputRecord>,
    /// `(region, provisionedModelName)` -> `ProvisionedModelArn`, so the other
    /// `ProvisionedModelId` spelling — the bare name — resolves to the Provisioned
    /// Throughput it names, and a second create of a live name is refused.
    ///
    /// Keyed by the region too because the ARN carries one, so the same name in another
    /// region names another Provisioned Throughput.
    pub provisioned_model_names: HashMap<(String, String), String>,
    /// `CreateProvisionedModelThroughput` idempotency: `(region, clientRequestToken)` ->
    /// the `provisionedModelArn` that token minted. Scoped to that one operation, and to
    /// the region the request addressed, the way the evaluation job index is.
    pub provisioned_model_create_tokens: HashMap<(String, String), String>,
    /// How many Provisioned Throughput ids this store has minted; the uniqueness half of
    /// a minted id.
    pub minted_provisioned_models: u64,
    /// Every stored custom model, keyed by its `CustomModelArn` — one of the two
    /// spellings a `ModelIdentifier` carries.
    pub custom_models: HashMap<String, CustomModelRecord>,
    /// `(region, modelName)` -> `CustomModelArn`, so the other `ModelIdentifier`
    /// spelling — the bare model name — resolves to the custom model it names, and a
    /// second create of a live name is refused.
    ///
    /// Keyed by the region too because the ARN carries one, so the same name in another
    /// region names another custom model.
    pub custom_model_names: HashMap<(String, String), String>,
    /// `CreateCustomModel` idempotency: `(region, clientRequestToken)` -> the `modelArn`
    /// that token minted. Scoped to that one operation, and to the region the request
    /// addressed, the way the Provisioned Throughput index is.
    pub custom_model_create_tokens: HashMap<(String, String), String>,
    /// How many custom model ids this store has minted; the uniqueness half of a minted
    /// id.
    pub minted_custom_models: u64,
    /// The foundation models this service offers, seeded from [`FOUNDATION_MODELS`] by
    /// [`FoundationModelCatalog::default`].
    ///
    /// A FIXED set, not a resource a caller creates: no operation of this service adds a
    /// row to it or takes one away, so it cannot grow the store either.
    pub foundation_models: FoundationModelCatalog,
    /// The account's acknowledged use case for model access: the `formData` bytes
    /// `PutUseCaseForModelAccess` last submitted, or `None` while the account has
    /// submitted none.
    ///
    /// ONE record, not a map, for the reason [`Self::model_invocation_logging`] is one:
    /// the operations that write and read it carry no identifier at all, so there is a
    /// single account-level form rather than a resource a caller can hold more than one
    /// of, and a further put REPLACES this field whole. The bytes are kept exactly as the
    /// put carried them, because the get reports that same `AcknowledgementFormDataBody`
    /// back.
    pub use_case_for_model_access: Option<Vec<u8>>,
}

impl ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState {
    /// The guardrail a `guardrailIdentifier` (bare id or full ARN) names.
    pub fn guardrail(&self, identifier: &str) -> Option<&Guardrail> {
        self.guardrails.get(guardrail_id_of(identifier))
    }

    /// The guardrail a `guardrailIdentifier` names, for mutation.
    pub fn guardrail_mut(&mut self, identifier: &str) -> Option<&mut Guardrail> {
        self.guardrails.get_mut(guardrail_id_of(identifier))
    }

    /// Mint a fresh `guardrailId`: `^[a-z0-9]+$`, unique within this store.
    ///
    /// The leading half is this store's mint count, so two ids can never collide;
    /// the trailing half is drawn from the creation instant, so an id is not a
    /// guessable sequence number.
    pub fn mint_guardrail_id(&mut self, now: SystemTime) -> String {
        self.minted_guardrails += 1;
        minted_id(self.minted_guardrails, now)
    }

    /// Mint a fresh `configId` for an enforced guardrail configuration:
    /// `AccountEnforcedGuardrailConfigurationId` is `^[a-z0-9]+$`, which is what
    /// [`minted_id`] spells, and the id is unique within this store for the two reasons a
    /// guardrail id is.
    pub fn mint_enforced_guardrail_config_id(&mut self, now: SystemTime) -> String {
        self.minted_enforced_guardrail_configs += 1;
        minted_id(self.minted_enforced_guardrail_configs, now)
    }

    /// The prompt router a `PromptRouterArn` names.
    pub fn prompt_router(&self, arn: &str) -> Option<&PromptRouter> {
        self.prompt_routers.get(arn)
    }

    /// Mint a fresh prompt router id: the resource half of a
    /// `default-prompt-router/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons: the leading half is this
    /// store's mint count so two ids can never collide, and the trailing half is drawn
    /// from the creation instant so an id is not a guessable sequence number. `[a-z0-9]+`
    /// is inside the `[a-zA-Z0-9-:.]+` the `PromptRouterArn` pattern admits there.
    pub fn mint_prompt_router_id(&mut self, now: SystemTime) -> String {
        self.minted_prompt_routers += 1;
        minted_id(self.minted_prompt_routers, now)
    }

    /// The marketplace model endpoint an `endpointArn` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status the endpoint holds at `now`, whichever operation asked.
    pub fn marketplace_model_endpoint(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut MarketplaceModelEndpointRecord> {
        let endpoint = self.marketplace_model_endpoints.get_mut(arn)?;
        endpoint.settle(now);
        Some(endpoint)
    }

    /// The `ModelImportJobArn` a `ModelImportJobIdentifier` names: the identifier itself
    /// when it is the ARN of a stored job, otherwise the ARN the job of that NAME in
    /// `region` carries.
    ///
    /// The identifier's pattern is the job ARN OR a bare `JobName`
    /// (`smithy_query(query="operation",operation="GetModelImportJob")`), so both
    /// spellings address the same job.
    #[must_use]
    pub fn model_import_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.model_import_jobs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.model_import_job_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The model import job a `ModelImportJobIdentifier` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status the job holds at `now`, whichever operation asked.
    pub fn model_import_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ModelImportJobRecord> {
        let arn = self.model_import_job_arn_of(identifier, region)?;
        let job = self.model_import_jobs.get_mut(&arn)?;
        job.complete(now);
        Some(job)
    }

    /// The ARN of the import job that imports the model an `ImportedModelIdentifier`
    /// names, or `None` when no stored job imports one.
    ///
    /// An `ImportedModelIdentifier` is the imported model's ARN or its bare NAME
    /// (`smithy_query(query="operation",operation="GetImportedModel")`), so both spellings
    /// address the same model, and a name names at most one imported model in an account
    /// and region. The imported model is not a record of its own: the job that imports it
    /// holds its identity, its data source and its key from the call that accepted the
    /// import onwards, so there is ONE copy of the model for every operation to read.
    /// The ARN spelling takes PRECEDENCE over the name, as
    /// [`Self::model_import_job_arn_of`] gives the job ARN precedence over the job name:
    /// the store is a `HashMap`, so a scan that accepted either match without an order
    /// would answer whichever record it reached first.
    #[must_use]
    pub fn importing_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        let mut named = None;
        for job in self.model_import_jobs.values() {
            if job.region != region {
                continue;
            }
            if job.imported_model_arn == identifier {
                return Some(job.arn.clone());
            }
            if job.imported_model_name == identifier {
                named = Some(job.arn.clone());
            }
        }
        named
    }

    /// Whether a stored job in `region` already imports a model under
    /// `imported_model_name`.
    #[must_use]
    pub fn imports_model_named(&self, region: &str, imported_model_name: &str) -> bool {
        self.importing_job_arn_of(imported_model_name, region)
            .is_some()
    }

    /// The imported model an `ImportedModelIdentifier` names, reported as the import job
    /// that holds it, together with the instant the import ENDED — the instant that model
    /// came into being — with any status transition the clock has made due at `now`
    /// materialised first, exactly as [`Self::model_import_job`] does it.
    ///
    /// `None` while that job has not reached [`IMPORT_JOB_STATUS_COMPLETED`]: importing is
    /// what CREATES the customized model ("A successful job transitions from in-progress to
    /// completed when the imported model is ready to use",
    /// `smithy_query(query="operation",operation="GetModelImportJob")`), so until the
    /// import has finished there is no imported model to report. That is also why the end
    /// instant is handed back BESIDE the record rather than left to a caller to unwrap:
    /// [`ModelImportJobRecord::complete`] sets it together with the terminal status, so a
    /// model this reports always has one.
    pub fn imported_model(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<(&ModelImportJobRecord, SystemTime)> {
        let arn = self.importing_job_arn_of(identifier, region)?;
        let job = self.model_import_jobs.get_mut(&arn)?;
        job.complete(now);
        let ended_at = job.ended_at?;
        (job.status == IMPORT_JOB_STATUS_COMPLETED).then_some((&*job, ended_at))
    }

    /// Mint a fresh model import job id: the resource half of a `model-import-job/<id>`
    /// ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `ModelImportJobArn`
    /// admits exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36
    /// halves spell.
    pub fn mint_model_import_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_import_jobs += 1;
        minted_id(self.minted_model_import_jobs, now)
    }

    /// Mint a fresh imported model id: the resource half of an `imported-model/<id>` ARN,
    /// unique within this store. `ImportedModelArn` admits `[a-z0-9]{12}` there, as the
    /// job ARN does.
    pub fn mint_imported_model_id(&mut self, now: SystemTime) -> String {
        self.minted_imported_models += 1;
        minted_id(self.minted_imported_models, now)
    }

    /// The evaluation job an `EvaluationJobIdentifier` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status it holds at `now`, whichever operation asked.
    pub fn evaluation_job(
        &mut self,
        identifier: &str,
        now: SystemTime,
    ) -> Option<&mut EvaluationJobRecord> {
        let job = self.evaluation_jobs.get_mut(identifier)?;
        job.settle(now);
        Some(job)
    }

    /// Mint a fresh evaluation job id: the resource half of an `evaluation-job/<id>` ARN,
    /// unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `EvaluationJobArn` admits
    /// exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36 halves
    /// spell.
    pub fn mint_evaluation_job_id(&mut self, now: SystemTime) -> String {
        self.minted_evaluation_jobs += 1;
        minted_id(self.minted_evaluation_jobs, now)
    }

    /// Mint a fresh Provisioned Throughput id: the resource half of a
    /// `provisioned-model/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `ProvisionedModelArn`
    /// admits exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36
    /// halves spell.
    pub fn mint_provisioned_model_id(&mut self, now: SystemTime) -> String {
        self.minted_provisioned_models += 1;
        minted_id(self.minted_provisioned_models, now)
    }

    /// The `ProvisionedModelArn` a `ProvisionedModelId` names: the identifier itself when
    /// it is the ARN of a stored Provisioned Throughput, otherwise the ARN the
    /// Provisioned Throughput of that NAME in `region` carries.
    ///
    /// The identifier's pattern is that ARN OR a bare `ProvisionedModelName`
    /// (`smithy_query(query="operation",operation="GetProvisionedModelThroughput")`), so
    /// both spellings address the same Provisioned Throughput.
    #[must_use]
    pub fn provisioned_model_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.provisioned_model_throughputs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.provisioned_model_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The Provisioned Throughput a `ProvisionedModelId` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status it holds at `now`, whichever operation asked.
    pub fn provisioned_model_throughput(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ProvisionedModelThroughputRecord> {
        let arn = self.provisioned_model_arn_of(identifier, region)?;
        let record = self.provisioned_model_throughputs.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every Provisioned Throughput status transition the clock has made due
    /// at `now`, so a LISTING reports — and filters on — the status each record holds at
    /// `now` exactly as a read of that one record would.
    pub fn settle_provisioned_model_throughputs(&mut self, now: SystemTime) {
        for record in self.provisioned_model_throughputs.values_mut() {
            record.settle(now);
        }
    }

    /// Mint a fresh custom model id: the resource half of a `custom-model/<base>/<id>`
    /// ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `CustomModelArn` admits
    /// exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36 halves
    /// spell.
    pub fn mint_custom_model_id(&mut self, now: SystemTime) -> String {
        self.minted_custom_models += 1;
        minted_id(self.minted_custom_models, now)
    }

    /// The `CustomModelArn` a `ModelIdentifier` names: the identifier itself when it is
    /// the ARN of a stored custom model, otherwise the ARN the custom model of that NAME
    /// in `region` carries.
    ///
    /// The identifier's pattern admits that ARN or a bare model name
    /// (`smithy_query(query="operation",operation="GetCustomModel")`), so both spellings
    /// address the same model.
    /// Either spelling resolves only to a model of `region`: the ARN carries a region, so
    /// one of another region names another account-region's model, exactly as the listing
    /// reports only this region's.
    #[must_use]
    pub fn custom_model_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if let Some(record) = self.custom_models.get(identifier) {
            return (record.region == region).then(|| identifier.to_string());
        }
        self.custom_model_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The custom model a `ModelIdentifier` names, with any status transition the clock
    /// has made due at `now` materialised first — so every caller reads the status the
    /// model holds at `now`, whichever operation asked.
    pub fn custom_model(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut CustomModelRecord> {
        let arn = self.custom_model_arn_of(identifier, region)?;
        let record = self.custom_models.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every custom model status transition the clock has made due at `now`,
    /// so a LISTING reports — and filters on — the status each record holds at `now`
    /// exactly as a read of that one model would.
    pub fn settle_custom_models(&mut self, now: SystemTime) {
        for record in self.custom_models.values_mut() {
            record.settle(now);
        }
    }

    /// Forget the Provisioned Throughput `arn` names, together with its name index entry
    /// and the idempotency tokens that minted it — so a delete leaves nothing behind for
    /// a later create of the same name to collide with, and the token map stays bounded by
    /// the LIVE Provisioned Throughputs.
    pub fn remove_provisioned_model_throughput(
        &mut self,
        arn: &str,
    ) -> Option<ProvisionedModelThroughputRecord> {
        let record = self.provisioned_model_throughputs.remove(arn)?;
        self.provisioned_model_names
            .remove(&(record.region.clone(), record.name.clone()));
        self.provisioned_model_create_tokens
            .retain(|_, minted| minted != arn);
        Some(record)
    }

    /// The resource a tagging call's `resourceARN` names, with every status transition the
    /// clock has made due at `now` materialised first — so a resource is tagged, and its
    /// tags reported, as it stands at `now`, exactly as a read of that one resource sees
    /// it.
    ///
    /// The ONE resolution of a tagging ARN, so every tagging operation reaches the same
    /// resource for the same value. A `TaggableResourcesArn` is the ARN of a resource of
    /// one of this service's families, or a bare resource NAME
    /// (`smithy_query(query="operation",operation="TagResource")`), and the resource TYPE
    /// inside the ARN picks the family; each family then resolves the value through the
    /// very index its own read operations resolve an identifier through, so tagging
    /// reaches exactly the resources those reads report. A value naming nothing this store
    /// holds resolves to `None`, whichever family it names.
    ///
    /// Two families are addressable under two ARN spellings, and both spellings name the
    /// same resource: a prompt router is `default-prompt-router/<id>` to `PromptRouterArn`
    /// and `prompt-router/<id>` here, and an imported model is `imported-model/<id>` to
    /// `ImportedModelArn` and `custom-model/imported/<id>` here. The second spelling is
    /// read back to the first — the one the store is keyed by — in the region the ARN
    /// itself carries, so neither spelling reaches another region's resource.
    ///
    /// The families this service holds no record of (a custom model, an inference profile,
    /// an automated reasoning policy, a model invocation job, an async invoke, a custom
    /// model deployment) resolve to `None`: nothing creates one yet, so this account holds
    /// no such resource to tag.
    pub fn tagged_resource(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<TaggedResource> {
        if resource_arn.contains(":guardrail/") {
            let id = guardrail_id_of(resource_arn).to_string();
            return self
                .guardrails
                .contains_key(&id)
                .then_some(TaggedResource::Guardrail(id));
        }
        if resource_arn.contains(":default-prompt-router/") {
            return self
                .prompt_routers
                .contains_key(resource_arn)
                .then(|| TaggedResource::PromptRouter(resource_arn.to_string()));
        }
        if let Some((_, id)) = resource_arn.rsplit_once(":prompt-router/") {
            let arn = prompt_router_arn(arn_region(resource_arn).unwrap_or(region), id);
            return self
                .prompt_routers
                .contains_key(&arn)
                .then_some(TaggedResource::PromptRouter(arn));
        }
        if resource_arn.contains(":model-import-job/") {
            let job = self.model_import_job(resource_arn, region, now)?;
            return Some(TaggedResource::ModelImportJob(job.arn.clone()));
        }
        if resource_arn.contains(":imported-model/") {
            let (job, _) = self.imported_model(resource_arn, region, now)?;
            return Some(TaggedResource::ImportedModel(job.arn.clone()));
        }
        if let Some((_, id)) = resource_arn.rsplit_once(":custom-model/imported/") {
            let spelled = imported_model_arn(arn_region(resource_arn).unwrap_or(region), id);
            let (job, _) = self.imported_model(&spelled, region, now)?;
            return Some(TaggedResource::ImportedModel(job.arn.clone()));
        }
        if resource_arn.contains(":evaluation-job/") {
            let job = self.evaluation_job(resource_arn, now)?;
            return Some(TaggedResource::EvaluationJob(job.arn.clone()));
        }
        if resource_arn.contains(":provisioned-model/") {
            let record = self.provisioned_model_throughput(resource_arn, region, now)?;
            return Some(TaggedResource::ProvisionedModelThroughput(
                record.arn.clone(),
            ));
        }
        if !resource_arn.starts_with("arn:") {
            // A bare resource NAME, which `TaggableResourcesArn` admits beside an ARN: it
            // names at most one resource of a family whose own identifier admits a name,
            // in this account and region, so each of those families is asked in turn.
            if let Some(job) = self.model_import_job(resource_arn, region, now) {
                return Some(TaggedResource::ModelImportJob(job.arn.clone()));
            }
            if let Some((job, _)) = self.imported_model(resource_arn, region, now) {
                return Some(TaggedResource::ImportedModel(job.arn.clone()));
            }
            if let Some(record) = self.provisioned_model_throughput(resource_arn, region, now) {
                return Some(TaggedResource::ProvisionedModelThroughput(
                    record.arn.clone(),
                ));
            }
        }
        None
    }

    /// The tags a resolved resource carries, read off the record that holds the resource —
    /// the same list the create that minted it filled and its own read reports, never a
    /// second copy.
    #[must_use]
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<&[Tag]> {
        match resource {
            TaggedResource::Guardrail(id) => {
                self.guardrails.get(id).map(|guardrail| &guardrail.tags)
            }
            TaggedResource::PromptRouter(arn) => {
                self.prompt_routers.get(arn).map(|router| &router.tags)
            }
            TaggedResource::ModelImportJob(arn) => {
                self.model_import_jobs.get(arn).map(|job| &job.job_tags)
            }
            TaggedResource::ImportedModel(arn) => self
                .model_import_jobs
                .get(arn)
                .map(|job| &job.imported_model_tags),
            TaggedResource::EvaluationJob(arn) => self.evaluation_jobs.get(arn).map(|job| &job.tags),
            TaggedResource::ProvisionedModelThroughput(arn) => self
                .provisioned_model_throughputs
                .get(arn)
                .map(|record| &record.tags),
        }
        .map(Vec::as_slice)
    }

    /// The tags a resolved resource carries, for a tagging call to rewrite in place.
    pub fn tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<Tag>> {
        match resource {
            TaggedResource::Guardrail(id) => self
                .guardrails
                .get_mut(id)
                .map(|guardrail| &mut guardrail.tags),
            TaggedResource::PromptRouter(arn) => self
                .prompt_routers
                .get_mut(arn)
                .map(|router| &mut router.tags),
            TaggedResource::ModelImportJob(arn) => self
                .model_import_jobs
                .get_mut(arn)
                .map(|job| &mut job.job_tags),
            TaggedResource::ImportedModel(arn) => self
                .model_import_jobs
                .get_mut(arn)
                .map(|job| &mut job.imported_model_tags),
            TaggedResource::EvaluationJob(arn) => {
                self.evaluation_jobs.get_mut(arn).map(|job| &mut job.tags)
            }
            TaggedResource::ProvisionedModelThroughput(arn) => self
                .provisioned_model_throughputs
                .get_mut(arn)
                .map(|record| &mut record.tags),
        }
    }

    /// The tags the resource a tagging call's `resourceARN` names carries, for REPORTING.
    pub fn resource_tags(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&[Tag]> {
        let resource = self.tagged_resource(resource_arn, region, now)?;
        self.tags_of(&resource)
    }

    /// The tags the resource a tagging call's `resourceARN` names carries, for WRITING.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut Vec<Tag>> {
        let resource = self.tagged_resource(resource_arn, region, now)?;
        self.tags_of_mut(&resource)
    }
}

/// A resource of this service that carries tags, as the record holding those tags is
/// keyed.
///
/// What [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::tagged_resource`]
/// resolves a tagging call's `resourceARN` to, so the ARN is read ONCE and every tagging
/// operation then reaches the same record. Each variant carries the key its family's store
/// is keyed by, which is why an imported model carries the ARN of the JOB that imports it:
/// that job is where the model's tags live, as [`ModelImportJobRecord`] states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A guardrail, by its `guardrailId`.
    Guardrail(String),
    /// A prompt router, by its `PromptRouterArn`.
    PromptRouter(String),
    /// A model import job, by its `ModelImportJobArn`.
    ModelImportJob(String),
    /// The model an import job imports, by that JOB's `ModelImportJobArn`.
    ImportedModel(String),
    /// An evaluation job, by its `EvaluationJobArn`.
    EvaluationJob(String),
    /// A Provisioned Throughput, by its `ProvisionedModelArn`.
    ProvisionedModelThroughput(String),
}

/// The region field of an ARN (`arn:<partition>:<service>:<region>:<account>:<resource>`),
/// or `None` when the value carries no such field.
fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3)
}

/// How many tags a resource holding `held` carries once `applied` has been associated with
/// it: the size of the UNION of their keys, because a key the resource already holds takes
/// the new value rather than becoming a second entry.
#[must_use]
pub fn tag_count_after(held: &[Tag], applied: &[Tag]) -> usize {
    let mut keys: BTreeSet<&str> = held.iter().map(|tag| tag.key.as_str()).collect();
    keys.extend(applied.iter().map(|tag| tag.key.as_str()));
    keys.len()
}

/// Associate `applied` with the tags a resource `held`, keyed by `key`: a key the resource
/// already carries takes the new value in place and a key it does not carry is appended,
/// so a resource carries one value per key and the order its keys were first associated in
/// is the order a read reports them.
pub fn apply_tags(held: &mut Vec<Tag>, applied: Vec<Tag>) {
    for tag in applied {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// A minted resource id: `^[a-z0-9]+$`, the store's mint `ordinal` then the sub-second
/// part of `now`, each in [`ID_HALF_WIDTH`] base-36 digits.
fn minted_id(ordinal: u64, now: SystemTime) -> String {
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos().into());
    format!(
        "{}{}",
        base36(ordinal, ID_HALF_WIDTH),
        base36(nanos, ID_HALF_WIDTH)
    )
}

/// What `UpdateMarketplaceModelEndpoint` answers as the `ResourceNotFoundException` message
/// for a Bedrock Marketplace model endpoint ARN it holds no endpoint for.
///
/// RECORDED behaviour, not a modelled sentence (the model's own doc for the error reads
/// "The specified resource Amazon Resource Name (ARN) was not found. …"): the corpus
/// `simparity-behavioral-ga-prod-v1` records Amazon Bedrock answering 404
/// `ResourceNotFoundException` with exactly this sentence for an `endpointArn` that names
/// no endpoint. A CONSTANT, interpolating no part of the request — it reports that the
/// resource is not there and nothing about which ARN was asked for, so the message cannot
/// be used to probe the store.
///
/// Scoped to the operation the corpus records it for. The same corpus records
/// `GetMarketplaceModelEndpoint` refusing the SAME ARN with 400 `ValidationException`
/// instead, so the code and the sentence are per-operation; and this service's guardrail
/// and prompt router handlers keep the sentences they already word, because no recording
/// here shows this one for those resources.
pub const MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE: &str = "Bedrock Resource not found.";

/// The endpoint NAME inside an `endpointArn`: the resource id of a
/// `.../endpoint/<name>` ARN, or the whole value when it is not one.
///
/// Built like [`guardrail_id_of`], and for the same reason: a member that carries a
/// resource's ARN is read back to the id the resource is named by.
pub fn marketplace_endpoint_name_of(endpoint_arn: &str) -> &str {
    match endpoint_arn.rsplit_once(":endpoint/") {
        Some((_, name)) => name,
        None => endpoint_arn,
    }
}

/// What `GetMarketplaceModelEndpoint` answers as its `ValidationException` message for an
/// `endpointArn` this account holds no endpoint for: `Could not find endpoint "<name>".`
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus `simparity-behavioral-ga-prod-v1` records Amazon Bedrock naming the endpoint the
/// request asked for — the resource id of the ARN the member carried
/// ([`marketplace_endpoint_name_of`]) — in exactly this sentence, quoted and closed with a
/// full stop. So the rule is "name the endpoint the ARN names", not one recorded name.
pub fn marketplace_endpoint_not_found_for_read(endpoint_arn: &str) -> String {
    format!(
        "Could not find endpoint \"{}\".",
        marketplace_endpoint_name_of(endpoint_arn)
    )
}

/// The `guardrailId` inside a `guardrailIdentifier`: the bare id, or the resource
/// half of a `.../guardrail/<id>` ARN.
pub fn guardrail_id_of(identifier: &str) -> &str {
    match identifier.rsplit_once(":guardrail/") {
        Some((_, id)) => id,
        None => identifier,
    }
}

/// The ARN of a guardrail in `region`:
/// `arn:aws:bedrock:<region>:<account>:guardrail/<id>`.
pub fn guardrail_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("guardrail/{id}"))
}

/// The ARN of a prompt router in `region`:
/// `arn:aws:bedrock:<region>:<account>:default-prompt-router/<id>`.
///
/// `default-prompt-router` is the resource type `PromptRouterArn` admits — its pattern
/// is `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:default-prompt-router/[a-zA-Z0-9-:.]+$`
/// (`smithy_query(query="shape",shape="PromptRouterArn")`) — so every router this
/// emulator mints is addressable through the members that carry that shape, whatever
/// `PromptRouterType` the router itself reports. `TaggableResourcesArn` spells the same
/// resource `prompt-router/[a-z0-9]{12}`; `PromptRouterArn` is the shape every
/// prompt-router operation carries, so it decides the form, and a minted id is 12
/// base-36 digits so it satisfies the other spelling's id form too.
pub fn prompt_router_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("default-prompt-router/{id}"))
}

/// The ARN of a Bedrock Marketplace model endpoint named `name` in `region`:
/// `arn:aws:sagemaker:<region>:<account>:endpoint/<name>`.
///
/// A Bedrock Marketplace model endpoint IS a SageMaker endpoint — `EndpointConfig`'s only
/// member is `sageMaker` (`smithy_query(query="shape",shape="EndpointConfig")`),
/// `SageMakerEndpoint` carries the instance count, instance type and execution role
/// SageMaker hosts it with, and `RegisterMarketplaceModelEndpoint` takes the ARN of an
/// endpoint that already exists — so the `endpointArn` every marketplace endpoint
/// operation carries is that SageMaker endpoint's ARN, whose resource id is the endpoint
/// name. `Arn` declares no pattern (`@length(max: 2048)` only), so nothing in the model
/// narrows it further.
pub fn marketplace_model_endpoint_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("sagemaker", region, &format!("endpoint/{name}"))
}

/// The `MarketplaceModelEndpoint` a response reports for a stored endpoint.
///
/// The ARN of a model import job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-import-job/<id>`, as `ModelImportJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-import-job/[a-z0-9]{12}$`)
/// spells it.
pub fn model_import_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("model-import-job/{id}"))
}

/// The ARN of an imported model in `region`:
/// `arn:aws:bedrock:<region>:<account>:imported-model/<id>`, as `ImportedModelArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:imported-model/[a-z0-9]{12}$`)
/// spells it.
pub fn imported_model_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("imported-model/{id}"))
}

/// The ARN of an evaluation job in `region`:
/// `arn:aws:bedrock:<region>:<account>:evaluation-job/<id>`, as `EvaluationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12}$`)
/// spells it.
pub fn evaluation_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("evaluation-job/{id}"))
}

/// The `EvaluationJobType` of a job configured by `config`.
///
/// `EvaluationConfig` is a union of `automated` ("an automated evaluation job that
/// computes metrics") and `human` ("an evaluation job that uses human workers")
/// (`smithy_query(query="shape",shape="EvaluationConfig")`), and `EvaluationJobType` is
/// `[Human, Automated]` — so the variant the request carried IS the job's type, and the
/// type is never read off the request's own wording.
///
/// Lives here rather than in one operation's file because `CreateEvaluationJob` derives it
/// and `GetEvaluationJob` reports it, so both must agree about it.
#[must_use]
pub fn evaluation_job_type_of(config: &EvaluationConfig) -> &'static str {
    match config {
        EvaluationConfig::Automated(_) => EVALUATION_JOB_TYPE_AUTOMATED,
        EvaluationConfig::Human(_) => EVALUATION_JOB_TYPE_HUMAN,
    }
}

/// The ARN of a Provisioned Throughput in `region`:
/// `arn:aws:bedrock:<region>:<account>:provisioned-model/<id>`, as `ProvisionedModelArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}$`)
/// spells it.
pub fn provisioned_model_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("provisioned-model/{id}"))
}

/// The base model id inside a `ModelIdentifier`: the `modelId` of a
/// `.../foundation-model/<modelId>` ARN, or the whole value when it is not one.
///
/// `ModelIdentifier` admits a model ARN or a bare model id
/// (`smithy_query(query="operation",operation="CreateProvisionedModelThroughput")`), so
/// both spellings name the same base model — built like [`guardrail_id_of`], and for the
/// same reason.
#[must_use]
pub fn base_model_id_of(model_identifier: &str) -> &str {
    match model_identifier.rsplit_once(":foundation-model/") {
        Some((_, model_id)) => model_id,
        None => model_identifier,
    }
}

/// Whether `model` is offered with Provisioned Throughput, which is what decides whether
/// a Provisioned Throughput can be created over it.
///
/// `InferenceType` is `[ON_DEMAND, PROVISIONED]` and
/// [`FoundationModel::inference_types_supported`] reports the ones that model is offered
/// under, so this reads the catalog rather than any property of the request.
#[must_use]
pub fn supports_provisioned_throughput(model: &FoundationModel) -> bool {
    model
        .inference_types_supported
        .contains(&INFERENCE_TYPE_PROVISIONED)
}

/// The `ProvisionedModelSummary` a listing reports for a stored Provisioned Throughput.
///
/// Lives here rather than in `ops/list_provisioned_model_throughputs.rs` because every
/// member it carries is also a member `GetProvisionedModelThroughput` reports, and the two
/// must agree about them: a listed summary is the same record the read of that one
/// Provisioned Throughput finds.
pub fn provisioned_model_summary(
    record: &ProvisionedModelThroughputRecord,
) -> ProvisionedModelSummary {
    ProvisionedModelSummary {
        provisioned_model_name: record.name.clone(),
        provisioned_model_arn: record.arn.clone(),
        model_arn: record.model_arn.clone(),
        desired_model_arn: record.desired_model_arn.clone(),
        foundation_model_arn: record.foundation_model_arn.clone(),
        model_units: record.model_units,
        desired_model_units: record.desired_model_units,
        status: record.status.to_string(),
        commitment_duration: record.commitment_duration.clone(),
        commitment_expiration_time: record.commitment_expiration_time.map(date_time),
        creation_time: date_time(record.created_at),
        last_modified_time: date_time(record.last_modified_at),
    }
}

/// The ARN of a custom model in `region`:
/// `arn:aws:bedrock:<region>:<account>:custom-model/<base>/<id>`, as `CustomModelArn`
/// (`^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|…)/[a-z0-9]{12}$`)
/// spells it.
///
/// `base` is the base-model segment that pattern carries before the id:
/// [`CUSTOM_MODEL_IMPORTED_BASE`] for a model whose artifacts were brought in, and the
/// base foundation model's `provider.model` name for one customized from a foundation
/// model.
pub fn custom_model_arn(region: &str, base: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("custom-model/{base}/{id}"))
}

/// The `CustomModelSummary` a listing reports for a stored custom model.
///
/// Lives here rather than in `ops/list_custom_models.rs` because every member it carries
/// is also a member `GetCustomModel` reports, and the two must agree about them: a listed
/// summary is the same record the read of that one model finds.
pub fn custom_model_summary(record: &CustomModelRecord) -> CustomModelSummary {
    CustomModelSummary {
        model_arn: record.arn.clone(),
        model_name: record.name.clone(),
        creation_time: date_time(record.created_at),
        base_model_arn: record.base_model_arn.clone(),
        base_model_name: record.base_model_name.clone(),
        customization_type: Some(record.customization_type.to_string()),
        // "The unique identifier of the account that owns the model": every model this
        // store holds was created by the one account the emulator presents.
        owner_account_id: Some(::engine::emulator_account_id().to_string()),
        model_status: Some(record.status.to_string()),
    }
}

/// Lives here rather than in one operation's file because
/// `Create`/`Get`/`UpdateMarketplaceModelEndpoint` all report this shape, and they must
/// report it alike: what a create or update answers is exactly what the next read finds.
pub fn marketplace_model_endpoint_view(
    endpoint: &MarketplaceModelEndpointRecord,
) -> MarketplaceModelEndpoint {
    MarketplaceModelEndpoint {
        endpoint_arn: endpoint.arn.clone(),
        model_source_identifier: endpoint.model_source_identifier.clone(),
        status: Some(endpoint.status.to_string()),
        // `statusMessage` is "additional information about the overall status, if
        // available"; a registered endpoint has none to add.
        status_message: None,
        created_at: date_time(endpoint.created_at),
        updated_at: date_time(endpoint.updated_at),
        endpoint_config: endpoint.endpoint_config.clone(),
        endpoint_status: endpoint.endpoint_status.to_string(),
        // `endpointStatusMessage` likewise: no failure or detail to report for an endpoint
        // that is coming up or serving.
        endpoint_status_message: None,
    }
}

/// Whether `value` is a `responseQualityDifference` Amazon Bedrock accepts: a multiple
/// of [`RESPONSE_QUALITY_DIFFERENCE_STEP`].
///
/// The modelled `@range(0..100)` is enforced ahead of any handler, so this is only about
/// the granularity inside that span. An exact remainder test: the step and every multiple
/// of it below 100 are representable in binary floating point, so a value that is a
/// multiple is one exactly.
#[must_use]
pub fn is_accepted_response_quality_difference(value: f64) -> bool {
    value % RESPONSE_QUALITY_DIFFERENCE_STEP == 0.0
}

/// A `GuardrailVersion`/`GuardrailNumericalVersion` as a number, or `None` when it
/// is not a numbered version.
pub fn version_number(version: &str) -> Option<u32> {
    if version.is_empty() || version.starts_with('0') {
        return None;
    }
    version.parse::<u32>().ok()
}

/// The AWS region this request was signed for, defaulting to `us-east-1` for an
/// unsigned one, and folded back to the default when it is not region-shaped
/// (`GuardrailArn` admits `[a-z0-9-]{1,20}` there).
pub fn request_region(parts: &http::request::Parts) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    let region = ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region);
    match region {
        Some(region)
            if (1..=20).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            region
        }
        _ => DEFAULT_REGION.to_string(),
    }
}

// =============================================================================
// This service's own check of a caller-supplied ARN
// =============================================================================
//
// For SOME of its resources, Amazon Bedrock reads an ARN a caller submits ITSELF, ahead
// of the constraints the model declares for the member carrying it, and answers its own
// short sentence for one it will not accept — never the `N validation error(s) detected:
// Value '…' at '…' failed to satisfy constraint: …` envelope a constraint violation
// words. The two sentences below, and which resources they cover, are recorded in
// `simparity-wire-aws-v1`; which of them a request gets is decided here, and only here,
// so every member carrying an ARN of those resources answers alike.

/// This service's own ARN namespace: the third field of every ARN it owns.
///
/// `aws.api#service$arnNamespace` on this model is `bedrock`
/// (`smithy_query(query="service_info")`), so an ARN whose vendor field is another
/// service's — an IAM role, a KMS key, an S3 bucket — names no resource of this service
/// and is left to the constraints the model declares for it.
pub const ARN_NAMESPACE: &str = "bedrock";

/// What this service answers for an ARN of a resource it validates itself that names a
/// region other than the one the request addresses.
///
/// RECORDED behaviour, not a modelled rule: the model declares no per-region ARN check,
/// and the corpus records Amazon Bedrock answering 400 `ValidationException` with exactly
/// this sentence for an ARN of one of the resources in
/// [`SELF_VALIDATED_RESOURCE_TYPES`] from another region. A CONSTANT, interpolating no
/// part of the request: it reports that the ARN is not usable here, and nothing about
/// which member carried it or which expression that member declares.
pub const ARN_INVALID_FOR_SERVICE_REGION: &str =
    "The provided ARN is invalid for the service region";

/// The ARN resource types this service validates ITSELF — the ones the two sentences
/// above cover.
///
/// A CATALOG, read off the corpus, because nothing in the model marks the difference and
/// the corpus records BOTH answers for ARNs of the same shape and the same foreign
/// region:
///
/// * these four resources answer this service's own sentences —
///   `custom-model` (`CreateCustomModelDeployment.modelArn`,
///   `CreateModelCopyJob.sourceModelArn`), `model-copy-job` (`GetModelCopyJob.jobArn`),
///   `model-invocation-job` (`Get`/`StopModelInvocationJob.jobIdentifier`) and
///   `provisioned-model` (`Get`/`Update`/`DeleteProvisionedModelThroughput
///   .provisionedModelId`);
/// * an `advanced-prompt-optimization-job` and an `evaluation-job` ARN from the SAME
///   foreign region, submitted by the same probe generator, are refused with the
///   framework's constraint envelope instead — recorded in
///   `issues/…-bedrock-validation-error-messages-render-model-regex-with.md`, which is
///   why this is a catalog of the resources bedrock checks itself rather than a rule over
///   every ARN. A resource type NOT listed here keeps the refusal the model's own
///   constraints word, which is what that recording shows.
///
/// Matched on the ARN's resource TYPE rather than on an operation or a member, so every
/// member of this service that carries one of these ARNs — the ones the recorded causes
/// name and the ones they do not (`GetCustomModel.modelIdentifier`,
/// `CreateProvisionedModelThroughput.modelId`, …) — is read the same way.
pub const SELF_VALIDATED_RESOURCE_TYPES: &[&str] = &[
    "custom-model",
    "model-copy-job",
    "model-invocation-job",
    "provisioned-model",
];

/// A caller-supplied ARN this service validates itself: one of its own namespace naming a
/// resource type in [`SELF_VALIDATED_RESOURCE_TYPES`], as the parsed ARN and the type it
/// names.
///
/// The parse is `service_common::arn`'s one parse — six fields, the first literally
/// `arn`, a non-empty partition, vendor and resource, the vendor compared exactly — so no
/// two services disagree about what an ARN is or which one is theirs.
fn self_validated_arn(value: &str) -> Option<(::service_common::arn::Arn<'_>, &'static str)> {
    let arn = ::service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    let resource_type = arn.resource_type();
    SELF_VALIDATED_RESOURCE_TYPES
        .iter()
        .find(|listed| **listed == resource_type)
        .map(|listed| (arn, *listed))
}

/// Whether `value` is an ARN of this service's own namespace naming a region other than
/// `region` — the region the request addresses, as
/// [`request_region`] reads it off the request's SigV4 credential scope.
///
/// Only an ARN of a resource this service validates itself
/// ([`SELF_VALIDATED_RESOURCE_TYPES`]), and only one whose region field is NOT empty: an
/// empty region names none to disagree with, which is how a partition-global resource is
/// written.
#[must_use]
pub fn arn_names_another_region(value: &str, region: &str) -> bool {
    self_validated_arn(value)
        .is_some_and(|(arn, _)| !arn.region.is_empty() && arn.region != region)
}

/// What this service answers for an ARN of its own namespace that is not a well-formed
/// ARN of the resource type IT names: `Invalid <resource type> ARN format`, the type
/// spelled in words.
///
/// The corpus records `Invalid custom model ARN format` for a `custom-model` ARN whose
/// resource id is not one `CustomModelArn` admits, so the sentence is built from the
/// resource type the ARN itself carries rather than from the member or the member's
/// expression — a `model-copy-job` ARN reports the model copy job.
///
/// `None` for an ARN of a resource this service does not validate itself, and for a value
/// that is no ARN of its namespace at all: those are values this check has nothing to say
/// about, so the refusal the model's own constraints word stands.
#[must_use]
pub fn invalid_arn_format_message(value: &str) -> Option<String> {
    let (_, resource_type) = self_validated_arn(value)?;
    // An ARN resource type is a hyphenated token (`custom-model`,
    // `model-invocation-job`); the recorded sentence spells it as words.
    let words = resource_type.replace('-', " ");
    Some(format!("Invalid {words} ARN format"))
}

/// The principal that made this request, for the `createdBy` / `updatedBy` an enforced
/// guardrail configuration reports.
///
/// Derived from the request's OWN SigV4 access key id, so the two members report who
/// called rather than a fabricated identity; the emulator sees an access key id and no
/// role session, so the id is rendered as the IAM principal that holds it. That id is
/// caller-controlled text, so it is first reduced to the characters an access key id can
/// hold, and an unsigned request falls back to the account root — the member is a plain
/// `String` in the model, and this keeps it an ARN whatever the header said.
#[must_use]
pub fn caller_principal(parts: &http::request::Parts) -> String {
    let user_name: String = ::engine::extract_akid(&parts.headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    if user_name.is_empty() {
        ::engine::emulator_arn("iam", "", "root")
    } else {
        ::engine::emulator_arn("iam", "", &format!("user/{user_name}"))
    }
}

/// A `SystemTime` as the wire timestamp type the response shapes carry.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

// =============================================================================
// The cursor every creation-time listing of this service pages with
// =============================================================================
//
// `ops/list_provisioned_model_throughputs.rs` and `ops/list_custom_models.rs` sort by the
// one instant their `sortBy` admits and page with a `nextToken`, so they need the SAME
// cursor: stated once here, because two renderings of it could drift and a token issued by
// one shape would then not resume the listing it came from.

/// The number of decimal digits a listing cursor's instant half carries.
///
/// Fixed-width so BYTE order over cursors IS the listing's order: nanoseconds since the
/// epoch need 19 digits for any instant this emulator can mint, and 20 leaves the width
/// stable past them.
pub const CURSOR_INSTANT_DIGITS: usize = 20;

/// The cursor of the row a listing resumes at: the instant the resource was created, then
/// its ARN.
///
/// Rendered so byte order over cursors is the `(creationTime, arn)` order such a listing is
/// sorted by — the instant half is fixed-width and `:` sorts below every character the rest
/// of an ARN can carry after it — which is what lets a page resume at "the first row at or
/// after this cursor" even when the row the previous page ended on has since been deleted.
/// It carries no whitespace, as `PaginationToken` (`^\S*$`) requires.
#[must_use]
pub fn listing_cursor(created_at: SystemTime, arn: &str) -> String {
    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos:0width$}:{arn}", width = CURSOR_INSTANT_DIGITS)
}

/// Whether `token` is a cursor [`listing_cursor`] renders, so a token from somewhere else
/// is refused rather than silently read as an empty or wrong page.
#[must_use]
pub fn cursor_belongs(token: &str) -> bool {
    let Some((instant, arn)) = token.split_once(':') else {
        return false;
    };
    instant.len() == CURSOR_INSTANT_DIGITS
        && instant.bytes().all(|byte| byte.is_ascii_digit())
        && !arn.is_empty()
}

/// `at` as epoch seconds, the scale a `creationTime` filter carries on the wire
/// (`@timestampFormat("date-time")`), so a filter and a record are compared on one scale.
#[must_use]
pub fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The `kmsKeyArn` a response reports for a request's `kmsKeyId`.
///
/// `kmsKeyId` admits a key ARN, a bare key id, or an alias, while `KmsKeyArn`
/// admits only `arn:aws:kms:<region>:<account>:key/<key id>`. A key ARN passes
/// through and a bare key id is completed into one; an ALIAS resolves to a key id
/// only inside KMS, so this reports no ARN for it rather than inventing one.
pub fn kms_key_arn(kms_key_id: Option<&str>, region: &str) -> Option<String> {
    let key_id = kms_key_id?;
    if key_id.starts_with("arn:") {
        return key_id.contains(":key/").then(|| key_id.to_string());
    }
    let is_key_id = key_id.len() == 36
        && key_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    is_key_id.then(|| ::engine::emulator_arn("kms", region, &format!("key/{key_id}")))
}

/// base-36 (`0-9a-z`) rendering of `value`, left-padded with `0` to `width`
/// digits.
fn base36(mut value: u64, width: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::with_capacity(width);
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// How a policy block's members are reported, which both conversions below and the
/// response construction in `ops/get_guardrail.rs` follow:
///
/// * A LIST member whose `@length` admits zero entries is always PRESENT — the empty list
///   when the request configured none — because the wire form of "nothing configured" for
///   such a member is `[]`, not an absent key. A list whose `@length` has `min: 1`
///   (`GuardrailContentPolicy$filters`, `GuardrailWordPolicy$words`,
///   `GuardrailSensitiveInformationPolicy$piiEntities`,
///   `GuardrailContextualGroundingPolicy$filters`) cannot be reported empty, so it stays
///   absent until the request configures it.
/// * A TIER member is always present on a block that models one, carrying the tier the
///   request named or [`TIER_CLASSIC`] when it named none.
fn topic_policy_of(config: &GuardrailTopicPolicyConfig) -> GuardrailTopicPolicy {
    GuardrailTopicPolicy {
        topics: config.topics_config.iter().map(topic_of).collect(),
        tier: Some(GuardrailTopicsTier {
            tier_name: config
                .tier_config
                .as_ref()
                .map_or_else(
                    || TIER_CLASSIC.to_string(),
                    |tier: &GuardrailTopicsTierConfig| tier.tier_name.clone(),
                ),
        }),
    }
}

fn topic_of(config: &GuardrailTopicConfig) -> GuardrailTopic {
    GuardrailTopic {
        name: config.name.clone(),
        definition: config.definition.clone(),
        // `GuardrailTopicExamples` is `@length(min: 0)`, so a topic with no examples
        // carries the empty list.
        examples: Some(config.examples.clone().unwrap_or_default()),
        r#type: Some(config.r#type.clone()),
        input_action: config.input_action.clone(),
        output_action: config.output_action.clone(),
        input_enabled: config.input_enabled,
        output_enabled: config.output_enabled,
    }
}

fn content_policy_of(config: &GuardrailContentPolicyConfig) -> GuardrailContentPolicy {
    GuardrailContentPolicy {
        filters: Some(
            config
                .filters_config
                .iter()
                .map(content_filter_of)
                .collect(),
        ),
        tier: Some(GuardrailContentFiltersTier {
            tier_name: config.tier_config.as_ref().map_or_else(
                || TIER_CLASSIC.to_string(),
                |tier: &GuardrailContentFiltersTierConfig| tier.tier_name.clone(),
            ),
        }),
    }
}

fn content_filter_of(config: &GuardrailContentFilterConfig) -> GuardrailContentFilter {
    GuardrailContentFilter {
        r#type: config.r#type.clone(),
        input_strength: config.input_strength.clone(),
        output_strength: config.output_strength.clone(),
        input_modalities: config.input_modalities.clone(),
        output_modalities: config.output_modalities.clone(),
        input_action: config.input_action.clone(),
        output_action: config.output_action.clone(),
        input_enabled: config.input_enabled,
        output_enabled: config.output_enabled,
    }
}

fn word_policy_of(config: &GuardrailWordPolicyConfig) -> GuardrailWordPolicy {
    GuardrailWordPolicy {
        words: config.words_config.as_ref().map(|words| {
            words
                .iter()
                .map(|word: &GuardrailWordConfig| GuardrailWord {
                    text: word.text.clone(),
                    input_action: word.input_action.clone(),
                    output_action: word.output_action.clone(),
                    input_enabled: word.input_enabled,
                    output_enabled: word.output_enabled,
                })
                .collect()
        }),
        managed_word_lists: Some(
            config
                .managed_word_lists_config
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|list: &GuardrailManagedWordsConfig| GuardrailManagedWords {
                    r#type: list.r#type.clone(),
                    input_action: list.input_action.clone(),
                    output_action: list.output_action.clone(),
                    input_enabled: list.input_enabled,
                    output_enabled: list.output_enabled,
                })
                .collect(),
        ),
    }
}

fn sensitive_information_policy_of(
    config: &GuardrailSensitiveInformationPolicyConfig,
) -> GuardrailSensitiveInformationPolicy {
    GuardrailSensitiveInformationPolicy {
        pii_entities: config.pii_entities_config.as_ref().map(|entities| {
            entities
                .iter()
                .map(|entity: &GuardrailPiiEntityConfig| GuardrailPiiEntity {
                    r#type: entity.r#type.clone(),
                    action: entity.action.clone(),
                    input_action: entity.input_action.clone(),
                    output_action: entity.output_action.clone(),
                    input_enabled: entity.input_enabled,
                    output_enabled: entity.output_enabled,
                })
                .collect()
        }),
        regexes: Some(
            config
                .regexes_config
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|regex: &GuardrailRegexConfig| GuardrailRegex {
                    name: regex.name.clone(),
                    description: regex.description.clone(),
                    pattern: regex.pattern.clone(),
                    action: regex.action.clone(),
                    input_action: regex.input_action.clone(),
                    output_action: regex.output_action.clone(),
                    input_enabled: regex.input_enabled,
                    output_enabled: regex.output_enabled,
                })
                .collect(),
        ),
    }
}

fn contextual_grounding_policy_of(
    config: &GuardrailContextualGroundingPolicyConfig,
) -> GuardrailContextualGroundingPolicy {
    GuardrailContextualGroundingPolicy {
        filters: config
            .filters_config
            .iter()
            .map(
                |filter: &GuardrailContextualGroundingFilterConfig| {
                    GuardrailContextualGroundingFilter {
                        r#type: filter.r#type.clone(),
                        threshold: filter.threshold,
                        action: filter.action.clone(),
                        enabled: filter.enabled,
                    }
                },
            )
            .collect(),
        tier: Some(GuardrailContextualGroundingTier {
            tier_name: config.tier_config.as_ref().map_or_else(
                || TIER_CLASSIC.to_string(),
                |tier: &GuardrailContextualGroundingTierConfig| tier.tier_name.clone(),
            ),
        }),
    }
}

fn automated_reasoning_policy_of(
    config: &GuardrailAutomatedReasoningPolicyConfig,
) -> GuardrailAutomatedReasoningPolicy {
    GuardrailAutomatedReasoningPolicy {
        policies: config.policies.clone(),
        confidence_threshold: config.confidence_threshold,
    }
}

/// The cross-region details for a `guardrailProfileIdentifier`, which may be a
/// profile id or a profile ARN.
///
/// `GuardrailCrossRegionGuardrailProfileId` is `<prefix>.guardrail.v<n>`; an
/// identifier that is not one of those names no profile this emulator can report,
/// so it reports none rather than inventing a profile.
fn cross_region_details_of(
    config: &GuardrailCrossRegionConfig,
    region: &str,
) -> Option<GuardrailCrossRegionDetails> {
    let identifier = config.guardrail_profile_identifier.as_str();
    let profile_id = match identifier.rsplit_once(":guardrail-profile/") {
        Some((_, id)) => id,
        None => identifier,
    };
    if !profile_id.contains(".guardrail.v") {
        return None;
    }
    Some(GuardrailCrossRegionDetails {
        guardrail_profile_id: Some(profile_id.to_string()),
        guardrail_profile_arn: Some(::engine::emulator_arn(
            "bedrock",
            region,
            &format!("guardrail-profile/{profile_id}"),
        )),
    })
}

// =============================================================================
// The published foundation model catalog
// =============================================================================

/// One foundation model Amazon Bedrock offers, as this control plane reports it.
///
/// Every field is `&'static` because the catalog is PUBLISHED data rather than anything a
/// request brings into being: this service has no operation that creates a foundation
/// model, so these rows are the same for every account and no handler writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundationModel {
    /// `BedrockModelId` — the id every operation that names a base model names it by.
    pub model_id: &'static str,
    /// `BrandedName` (len 1..20) — the model's own name.
    pub model_name: &'static str,
    /// `BrandedName` (len 1..20) — the provider that publishes the model.
    pub provider_name: &'static str,
    /// `ModelModalityList` — what the model takes, each value out of `ModelModality`
    /// (`crate::ops::types::MODEL_MODALITY_VALUES`).
    pub input_modalities: &'static [&'static str],
    /// `ModelModalityList` — what the model produces, from the same value set.
    pub output_modalities: &'static [&'static str],
    /// Whether the model streams its response.
    pub response_streaming_supported: bool,
    /// `ModelCustomizationList` — the customizations the model supports, each value out of
    /// `ModelCustomization` (`crate::ops::types::MODEL_CUSTOMIZATION_VALUES`); empty for a
    /// model that supports none.
    pub customizations_supported: &'static [&'static str],
    /// `InferenceTypeList` — the inference types the model is offered under, each value out
    /// of `InferenceType` (`crate::ops::types::INFERENCE_TYPE_VALUES`).
    pub inference_types_supported: &'static [&'static str],
    /// `FoundationModelLifecycleStatus` — `ACTIVE` for a model that is available, `LEGACY`
    /// for one that is deprecated
    /// (`crate::ops::types::FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES`).
    pub lifecycle_status: &'static str,
}

/// `ModelModality` — a model that takes text only.
const IN_TEXT: &[&str] = &["TEXT"];
/// `ModelModality` — a model that takes text or an image.
const IN_TEXT_AND_IMAGE: &[&str] = &["TEXT", "IMAGE"];
/// `ModelModality` — a model that generates text.
const OUT_TEXT: &[&str] = &["TEXT"];
/// `ModelModality` — a model that generates an image.
const OUT_IMAGE: &[&str] = &["IMAGE"];
/// `ModelModality` — a model that produces an embedding vector.
const OUT_EMBEDDING: &[&str] = &["EMBEDDING"];
/// `InferenceType` — a model offered on demand only.
const ON_DEMAND: &[&str] = &["ON_DEMAND"];
/// `InferenceType` — a model offered both on demand and with Provisioned Throughput.
const ON_DEMAND_AND_PROVISIONED: &[&str] = &["ON_DEMAND", "PROVISIONED"];
/// `ModelCustomizationList` — a model no customization job accepts.
const NO_CUSTOMIZATION: &[&str] = &[];
/// `ModelCustomization` — a model that can be fine-tuned on labelled data.
const FINE_TUNING: &[&str] = &["FINE_TUNING"];
/// `ModelCustomization` — a model that accepts both fine-tuning and continued
/// pre-training.
const FINE_TUNING_AND_PRE_TRAINING: &[&str] = &["FINE_TUNING", "CONTINUED_PRE_TRAINING"];
/// `ModelCustomization` — a model that accepts both fine-tuning and distillation.
const FINE_TUNING_AND_DISTILLATION: &[&str] = &["FINE_TUNING", "DISTILLATION"];
/// `FoundationModelLifecycleStatus` — the model is available.
const LIFECYCLE_ACTIVE: &str = "ACTIVE";
/// `FoundationModelLifecycleStatus` — the model is deprecated: its id still resolves, and
/// callers are expected to move off it.
const LIFECYCLE_LEGACY: &str = "LEGACY";

/// Every foundation model this emulator's Amazon Bedrock offers, in `modelId` order.
///
/// Amazon Bedrock publishes its base models rather than letting a caller create one —
/// there is no `CreateFoundationModel` — so this is the published catalog itself: the set
/// `ListFoundationModels` reports and `GetFoundationModel` resolves one row of, not
/// anything a request can add to or remove from.
///
/// The ids are the base model ids AWS publishes in "Amazon Bedrock base model IDs
/// (on-demand throughput)" and "Supported foundation models in Amazon Bedrock", and the
/// rows past their end of life carry the ids "Model lifecycle" lists; this emulator's
/// bedrock-runtime plane recognises the same ids, so the two planes agree about which
/// models exist. Each row's provider, modalities, streaming support, customizations and
/// inference types are as those pages publish them for that model; a per-model attribute
/// no page publishes is left out of the row rather than invented (the lifecycle's
/// `startOfLifeTime`, `endOfLifeTime`, `legacyTime` and `publicExtendedAccessTime` are
/// such instants, so a listed model reports its `status` and no dates).
///
/// It is a SUBSET of what AWS offers, spanning every value of each member a caller can
/// filter on, because the filter rule — not the catalog's size — is what this emulator
/// has to get right.
pub const FOUNDATION_MODELS: &[FoundationModel] = &[
    FoundationModel {
        model_id: "ai21.j2-ultra-v1",
        model_name: "Jurassic-2 Ultra",
        provider_name: "AI21 Labs",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_LEGACY,
    },
    FoundationModel {
        model_id: "ai21.jamba-1-5-mini-v1:0",
        model_name: "Jamba 1.5 Mini",
        provider_name: "AI21 Labs",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.nova-canvas-v1:0",
        model_name: "Nova Canvas",
        provider_name: "Amazon",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.nova-lite-v1:0",
        model_name: "Nova Lite",
        provider_name: "Amazon",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.nova-micro-v1:0",
        model_name: "Nova Micro",
        provider_name: "Amazon",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.nova-pro-v1:0",
        model_name: "Nova Pro",
        provider_name: "Amazon",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.titan-embed-image-v1",
        model_name: "Titan Embed Image",
        provider_name: "Amazon",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.titan-embed-text-v2:0",
        model_name: "Titan Embed Text V2",
        provider_name: "Amazon",
        input_modalities: IN_TEXT,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.titan-image-generator-v1",
        model_name: "Titan Image Gen G1",
        provider_name: "Amazon",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.titan-text-express-v1",
        model_name: "Titan Text Express",
        provider_name: "Amazon",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_PRE_TRAINING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_LEGACY,
    },
    FoundationModel {
        model_id: "amazon.titan-text-lite-v1",
        model_name: "Titan Text Lite",
        provider_name: "Amazon",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_PRE_TRAINING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "amazon.titan-text-premier-v1:0",
        model_name: "Titan Text Premier",
        provider_name: "Amazon",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        model_name: "Claude 3.5 Sonnet",
        provider_name: "Anthropic",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        model_name: "Claude 3 Haiku",
        provider_name: "Anthropic",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
        model_name: "Claude 3 Sonnet",
        provider_name: "Anthropic",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "anthropic.claude-v2",
        model_name: "Claude",
        provider_name: "Anthropic",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_LEGACY,
    },
    FoundationModel {
        model_id: "cohere.command-light-text-v14",
        model_name: "Command Light",
        provider_name: "Cohere",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "cohere.command-text-v14",
        model_name: "Command",
        provider_name: "Cohere",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "cohere.embed-english-v3",
        model_name: "Embed English",
        provider_name: "Cohere",
        input_modalities: IN_TEXT,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "meta.llama2-70b-chat-v1",
        model_name: "Llama 2 Chat 70B",
        provider_name: "Meta",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_LEGACY,
    },
    FoundationModel {
        model_id: "meta.llama3-1-8b-instruct-v1:0",
        model_name: "Llama 3.1 8B",
        provider_name: "Meta",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "meta.llama3-70b-instruct-v1:0",
        model_name: "Llama 3 70B Instruct",
        provider_name: "Meta",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "mistral.mistral-large-2402-v1:0",
        model_name: "Mistral Large",
        provider_name: "Mistral AI",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "mistral.mixtral-8x7b-instruct-v0:1",
        model_name: "Mixtral 8x7B",
        provider_name: "Mistral AI",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
    FoundationModel {
        model_id: "stability.stable-diffusion-xl-v1",
        model_name: "SDXL 1.0",
        provider_name: "Stability AI",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        lifecycle_status: LIFECYCLE_ACTIVE,
    },
];

/// The catalog as the per-service store holds it: one entry per `modelId`, keyed by it, so
/// iteration is in `modelId` order and a lookup by id is one map probe.
///
/// A newtype so it SEEDS ITSELF ([`Default`]) out of [`FOUNDATION_MODELS`]: the store
/// around it keeps its derived `Default`, and every handler that reports or resolves a
/// base model reads the catalog through the engine like any other part of this service's
/// state rather than reaching for the constant.
#[derive(Debug, Clone)]
pub struct FoundationModelCatalog(BTreeMap<&'static str, FoundationModel>);

impl Default for FoundationModelCatalog {
    fn default() -> Self {
        Self(
            FOUNDATION_MODELS
                .iter()
                .map(|model| (model.model_id, *model))
                .collect(),
        )
    }
}

impl FoundationModelCatalog {
    /// Every model in the catalog, in `modelId` order.
    pub fn models(&self) -> impl Iterator<Item = &FoundationModel> {
        self.0.values()
    }

    /// The model a `modelId` names, or `None` when the catalog carries no such model.
    #[must_use]
    pub fn model(&self, model_id: &str) -> Option<&FoundationModel> {
        self.0.get(model_id)
    }
}

/// The `foundation-model` ARN of `model_id` in `region`:
/// `arn:aws:bedrock:<region>::foundation-model/<modelId>`.
///
/// Built here rather than through `::engine::emulator_arn` because `FoundationModelArn` is
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/…$` — an EMPTY account
/// field, since a foundation model is published by AWS rather than owned by the account
/// reading it, so the canonical account id that helper interpolates would put this ARN
/// outside the pattern the model declares for it.
#[must_use]
pub fn foundation_model_arn(region: &str, model_id: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::foundation-model/{model_id}")
}

/// Whether `by_provider` — a `ListFoundationModels` `byProvider` filter — names `model`'s
/// provider.
///
/// Accepted spellings: the provider's own `providerName` as the catalog reports it, and the
/// provider segment of the `modelId` (everything before its first `.`), which is the
/// lowercase spelling the AWS CLI's own examples pass. Compared case-insensitively,
/// because `Provider` declares a pattern (`^[A-Za-z0-9- ]{1,63}$`) and no case.
#[must_use]
pub fn provider_matches(model: &FoundationModel, by_provider: &str) -> bool {
    let id_provider = model
        .model_id
        .split_once('.')
        .map_or(model.model_id, |(provider, _)| provider);
    by_provider.eq_ignore_ascii_case(model.provider_name)
        || by_provider.eq_ignore_ascii_case(id_provider)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState>)
}
