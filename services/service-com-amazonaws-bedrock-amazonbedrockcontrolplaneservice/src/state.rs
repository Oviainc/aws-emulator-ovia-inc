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
//! And it holds the prompt routers `ops/create_prompt_router.rs` mints,
//! `ops/get_prompt_router.rs` reads back, `ops/list_prompt_routers.rs` reports and
//! `ops/delete_prompt_router.rs` removes, so a router created by one request is the
//! router every later request sees and a router one request deleted is gone for every
//! later one.
//!
//! And it holds the model import jobs `ops/create_model_import_job.rs` mints and
//! `ops/get_model_import_job.rs` reads back, together with the instant each job's
//! non-terminal `status` is owed its move to `Completed`. Those same records hold the
//! models those jobs import, which `ops/get_imported_model.rs` reads back once the
//! import has completed — the job that imports a model is where that model's identity,
//! data source and key live, so there is only ever one copy of either.
//!
//! And it holds the evaluation jobs `ops/create_evaluation_job.rs` mints and
//! `ops/get_evaluation_job.rs` reads back, together with the instant each job's
//! non-terminal status is owed its move to a terminal one and the instant each job
//! `ops/batch_delete_evaluation_job.rs` accepted for deletion is owed its removal.
//!
//! And it holds the advanced prompt optimization jobs
//! `ops/create_advanced_prompt_optimization_job.rs` mints,
//! `ops/get_advanced_prompt_optimization_job.rs` reads back and
//! `ops/stop_advanced_prompt_optimization_job.rs` winds down, together with the instant
//! each job's non-terminal `jobStatus` is owed its move to a terminal one.
//!
//! And it holds the model customization jobs `ops/create_model_customization_job.rs`
//! mints, `ops/get_model_customization_job.rs` reads back and
//! `ops/stop_model_customization_job.rs` winds down, together with the instant each job's
//! non-terminal `status` is owed its move to a terminal one. A job that reaches
//! `Completed` is what brings the custom model it was customizing into being, so the
//! completion registers that model in the custom models below rather than holding a second
//! copy of it: the model a job produced is the model `ops/get_custom_model.rs` and
//! `ops/list_custom_models.rs` report.
//!
//! And it holds the model copy jobs `ops/create_model_copy_job.rs` submits and
//! `ops/get_model_copy_job.rs` reads back, together with the instant each job's
//! non-terminal `status` is owed its move to `Completed`. A job that completes is what
//! brings the COPIED model into being, so the completion registers that model in the
//! custom models below rather than holding a second copy of it: the model a copy produced
//! is the model `ops/get_custom_model.rs` and `ops/list_custom_models.rs` report.
//!
//! And it holds the Bedrock Marketplace model endpoints
//! `ops/create_marketplace_model_endpoint.rs` mints,
//! `ops/update_marketplace_model_endpoint.rs` reconfigures,
//! `ops/register_marketplace_model_endpoint.rs` registers with Amazon Bedrock,
//! `ops/get_marketplace_model_endpoint.rs` reads back and
//! `ops/deregister_marketplace_model_endpoint.rs` drops, together with the instant each
//! endpoint's non-terminal `endpointStatus` is owed its move to `InService`.
//!
//! And it holds the Provisioned Throughputs `ops/create_provisioned_model_throughput.rs`
//! mints, `ops/get_provisioned_model_throughput.rs` reads back,
//! `ops/list_provisioned_model_throughputs.rs` reports and
//! `ops/delete_provisioned_model_throughput.rs` removes, together with the instant each
//! one's non-terminal `status` is owed its move to `InService`.
//!
//! And it holds the custom models `ops/create_custom_model.rs` mints,
//! `ops/get_custom_model.rs` reads back, `ops/list_custom_models.rs` reports and
//! `ops/delete_custom_model.rs` removes, together with the instant each one's non-terminal
//! `modelStatus` is owed its move to `Active`.
//!
//! And it holds the custom model deployments `ops/create_custom_model_deployment.rs`
//! mints, `ops/get_custom_model_deployment.rs` reads back and
//! `ops/list_custom_model_deployments.rs` reports, together with the instant each
//! deployment's non-terminal `status` is owed its move to `Active`.
//!
//! And it holds the Automated Reasoning policies `ops/create_automated_reasoning_policy.rs`
//! mints, `ops/update_automated_reasoning_policy.rs` rewrites and
//! `ops/get_automated_reasoning_policy.rs` reads back, each with the draft definition it
//! was last written with and the hash of that definition, so the hash the write answered
//! is the hash the read reports — and, beside that draft, every numbered version
//! `ops/create_automated_reasoning_policy_version.rs` cuts from it, which the same read
//! reports when the ARN it is given carries that version's number, unchanged by any later
//! write to the draft. `ops/list_automated_reasoning_policies.rs` reports those same
//! revisions as summaries — every policy's draft, or the revisions of the one policy an ARN
//! filter names — so a listing reports what a read of each revision finds.
//! `ops/export_automated_reasoning_policy_version.rs` reads the
//! DEFINITION of whichever of those revisions the ARN names, so what a revision was written
//! with is what the export reports.
//! `ops/delete_automated_reasoning_policy.rs` removes whichever of those revisions the ARN
//! names — one numbered version, or the policy with every artifact of it held below — so a
//! revision one request deleted is a revision no later request finds.
//!
//! And it holds the build workflows of those policies —
//! `ops/start_automated_reasoning_policy_build_workflow.rs` starts them,
//! `ops/cancel_automated_reasoning_policy_build_workflow.rs` stops them and
//! `ops/get_automated_reasoning_policy_build_workflow.rs` reads them back — each with the
//! document the start named and the instant each status of the build's progression —
//! `PREPROCESSING`, `BUILDING`, `TESTING` and then `COMPLETED` — is owed at, or the instant
//! `CANCELLED` is owed once a cancel has asked for it.
//!
//! And it holds the Automated Reasoning policy tests
//! `ops/create_automated_reasoning_policy_test_case.rs` mints,
//! `ops/get_automated_reasoning_policy_test_case.rs` reads back and
//! `ops/delete_automated_reasoning_policy_test_case.rs` removes, each keyed by the policy it
//! was created under, so a test created by one request is the test every later request
//! sees — and a test one request deleted is a test no later request finds.
//!
//! And it holds the runs of those tests —
//! `ops/start_automated_reasoning_policy_test_workflow.rs` schedules them against a built
//! build workflow and `ops/get_automated_reasoning_policy_test_result.rs` reads each one
//! back — each keyed by the policy, the build workflow and the test it runs, with the
//! instant its non-terminal `testRunStatus` is owed its move to `COMPLETED`.
//!
//! And it holds the account's acknowledged use case for model access — the single form
//! `ops/put_use_case_for_model_access.rs` submits and `ops/get_use_case_for_model_access.rs`
//! reads back, so the form one request submits is the form the next request gets.
//!
//! And it holds the account's data retention setting — the single record
//! `ops/put_account_data_retention.rs` stores and `ops/get_account_data_retention.rs` reads
//! back, so the mode one request sets is the mode the next request reports.
//!
//! And it holds the application inference profiles `ops/create_inference_profile.rs`
//! mints, `ops/get_inference_profile.rs` reads back,
//! `ops/list_inference_profiles.rs` reports and `ops/delete_inference_profile.rs`
//! removes, each holding the foundation models that profile tracks metrics and costs for.
//!
//! And it holds the batch inference jobs `ops/create_model_invocation_job.rs` submits,
//! `ops/get_model_invocation_job.rs` reads back, `ops/list_model_invocation_jobs.rs`
//! reports and `ops/stop_model_invocation_job.rs` winds down, together with the instant
//! each job's non-terminal `status` is owed its next move and the instant the job would
//! time out. Each of those records also holds what the job's WORK established — the records
//! read out of its input object and the tally of the run that invoked the model over them
//! ([`model_invocation_run`]) — because the counts the two reads report and the results
//! stored under the job's output prefix are two sides of one run.
//!
//! And it holds the published foundation model catalog — the base models
//! `ops/list_foundation_models.rs` reports and `ops/get_foundation_model.rs` resolves one
//! row of — which is seeded with the store and never written, because no operation of this
//! service creates a foundation model. The cross-region inference profiles Amazon Bedrock
//! publishes over those models ([`system_defined_inference_profiles`], which
//! `ops/list_inference_profiles.rs` reports beside the account's own profiles) are
//! published the same way, so they are constants rather than part of the store at all.
//!
//! And it holds the resource policies `ops/put_resource_policy.rs` attaches,
//! `ops/get_resource_policy.rs` reports and `ops/delete_resource_policy.rs` takes away,
//! keyed by the [`TaggedResource`] each one is
//! attached to — one of the resources above, resolved out of the very same maps, so a
//! policy is attached to a RESOURCE rather than to one spelling of its ARN.
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
//!
//! This module is a DIRECTORY: the store struct, the guardrail family, the quotas and the
//! shared helpers stay in this file, and the families large enough to read on their own —
//! the stored records of the model, endpoint and job resources ([`resource_records`]), the
//! Automated Reasoning policy family ([`automated_reasoning_policies`]), the foundation
//! model catalogue ([`foundation_models`]) and the store's own lookups
//! ([`store_access`]) — live in their own files beside it. Every one of them is
//! re-exported here, so each name is still reached as `crate::state::…` and the handlers
//! address ONE module.


use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

/// The INFERENCE plane's account-enforced guardrail: the one home of the enforcement an
/// installed configuration puts in force, which this control plane writes on the put that
/// installs it ([`enforced_guardrail_enforcement`]) and the Bedrock Runtime emulator's
/// invocation surfaces read.
use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::enforced_guardrail as runtime_guardrail;

use crate::ops::types::{
    AdvancedPromptOptimizationInputConfig, AdvancedPromptOptimizationOutputConfig,
    AutomatedReasoningPolicyDefinition, AutomatedReasoningPolicyTestCase,
    AutomatedReasoningPolicyTestResult, CustomModelDataSource,
    CustomModelDeploymentSummary,
    CapabilitySupport,
    BatchSupport,
    CustomModelSummary, CustomizationConfig, EndpointConfig, EvaluationConfig,
    ConverseSupport, FeaturesSupport,
    ModelInvocationJobInputDataConfig, ModelInvocationJobOutputDataConfig,
    ModelInvocationJobSummary,
    EvaluationInferenceConfig, EvaluationOutputDataConfig, FoundationModelDetails,
    FoundationModelLifecycle, FoundationModelSummary, GuardrailAutomatedReasoningPolicy,
    GuardrailAutomatedReasoningPolicyConfig,
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
    GuardrailWordPolicyConfig, InferenceProfileModel, InferenceProfileSummary, LegalTerm,
    LoggingConfig,
    MarketplaceModelEndpoint,
    ModelConfiguration, ModelCopyJobSummary, ModelDataSource, ModelEnforcement, Offer,
    OutputDataConfig,
    PricingTerm, PromptRouterSummary, PromptRouterTargetModel,
    ProvisionedModelSummary, RoutingCriteria, SelectiveContentGuarding, SupportTerm, Tag,
    TermDetails, TrainingDataConfig,
    ValidationDataConfig, VpcConfig,
};

/// The WORK a batch inference job performs: the records it reads out of its input object,
/// the model invocation it performs over each one and the result records it writes under
/// its output prefix. It lives beside this module rather than in it because it reaches
/// another emulated service (object storage, and the inference surface), while the record
/// and its lifecycle live here.
pub mod model_invocation_run;

/// Whether the Amazon S3 location a model source names satisfies the Amazon-managed-bucket
/// pre-condition the member carrying it states. Beside this module for the reason
/// [`model_invocation_run`] is: deciding it means asking another emulated service (object
/// storage) whose bucket a name is, while the records the decision guards live here.
pub mod model_source;

/// The foundation-model catalogue this emulator publishes, and the renderings the model
/// and inference profile surfaces report it through. Here rather than in this file so one
/// file holds one concern; every name it declares is re-exported below, so a handler still
/// reaches it as `crate::state::…`.
mod foundation_models;

/// The lookups of [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState`] — the
/// inherent `impl` the handlers reach the store through, beside the struct it belongs to.
mod store_access;

/// The Automated Reasoning policy family: its quotas, statuses, spans, refusal sentences
/// and stored records.
mod automated_reasoning_policies;

/// The stored record of each model, endpoint and job resource, with the lifecycle its own
/// clock owes it.
mod resource_records;

pub use automated_reasoning_policies::*;
pub use foundation_models::*;
pub use resource_records::*;

/// The working version every guardrail always has, as `GuardrailDraftVersion`
/// (`^DRAFT$`) spells it.
///
/// An Automated Reasoning policy carries the same spelling for its own working version:
/// `CreateAutomatedReasoningPolicyResponse$version` documents "The initial version is
/// always DRAFT"
/// (`smithy_query(query="operation",operation="CreateAutomatedReasoningPolicy")`), so the
/// two resources name their draft with ONE constant rather than two spellings that could
/// drift.
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

/// What this service answers as its `ResourceNotFoundException` message for a
/// `guardrailIdentifier` that resolves to no guardrail of this account — a guardrail that
/// was never created, one that has been deleted, or a version the guardrail does not
/// carry.
///
/// RECORDED behaviour: Amazon Bedrock reports that the guardrail is not there and nothing
/// about the request — a FIXED sentence, carrying no identifier, no version and no account
/// reference — so every operation that resolves a guardrail identifier answers with this
/// one value rather than wording the miss itself.
pub const GUARDRAIL_NOT_FOUND: &str = "This guardrail does not exist.";

/// What this service answers as its `ResourceNotFoundException` message when the guardrail
/// a request's own BODY names — the `guardrailInferenceConfig` an enforced guardrail
/// configuration is installed from — resolves to no guardrail revision of this account.
///
/// RECORDED behaviour: Amazon Bedrock words this miss as the guardrail "provided in the
/// request" rather than with the sentence an operation ADDRESSED by a guardrail identifier
/// answers ([`GUARDRAIL_NOT_FOUND`]): the two are different sentences of the service, and a
/// caller reading the message learns which half of its request named the missing guardrail.
/// Like that one it is wholly FIXED — no identifier, no version, no account reference.
pub const REQUEST_GUARDRAIL_NOT_FOUND: &str =
    "The guardrail provided in the request does not exist.";

/// The collections of guardrail policy entries whose modelled maximum is a per-guardrail
/// QUOTA, each with the sentence Amazon Bedrock refuses a request carrying more entries than
/// that with.
///
/// RECORDED behaviour: a guardrail policy's entries are bounded by a service quota, and
/// going past one is reported as that quota — a FIXED sentence naming the entries and the
/// policy holding them, carrying neither the count submitted nor the limit — rather than
/// through the generic member-length clause [`crate::service::validation`] words a modelled
/// `@length` violation with, so a caller can tell "you sent too many entries" apart from
/// "one entry is malformed".
///
/// Every list member of the guardrail policy configurations the model declares a MAXIMUM
/// for, keyed by the member path the constraint walk reports it at
/// (`smithy_query(query="shape",shape="GuardrailTopicPolicyConfig,GuardrailContentPolicyConfig,GuardrailWordPolicyConfig,GuardrailSensitiveInformationPolicyConfig,GuardrailAutomatedReasoningPolicyConfig")`):
/// `topicsConfig` 1..30, `filtersConfig` 1..6, `wordsConfig` 1..10000, `regexesConfig`
/// 1..30 and `policies` 1..2. The members those same shapes declare with NO maximum —
/// `piiEntitiesConfig`, `managedWordListsConfig` and the contextual grounding policy's
/// `filtersConfig` — bound no count, so no request can exceed one and none is listed here.
/// A collection's MINIMUM is not a quota (an empty list is a malformed request, not too much
/// of one), so it keeps the member-length clause.
pub const GUARDRAIL_POLICY_ENTRY_QUOTAS: &[(&str, &str)] = &[
    (
        "topicPolicyConfig.topicsConfig",
        "Number of topics in topic policy exceeds quota limit.",
    ),
    (
        "contentPolicyConfig.filtersConfig",
        "Number of filters in content policy exceeds quota limit.",
    ),
    (
        "wordPolicyConfig.wordsConfig",
        "Number of words in word policy exceeds quota limit.",
    ),
    (
        "sensitiveInformationPolicyConfig.regexesConfig",
        "Number of regexes in sensitive information policy exceeds quota limit.",
    ),
    (
        "automatedReasoningPolicyConfig.policies",
        "Number of policies in automated reasoning policy exceeds quota limit.",
    ),
];

/// The quota sentence for the guardrail policy collection at `member`, or `None` when that
/// member bounds no quota.
#[must_use]
pub fn guardrail_policy_entry_quota_message(member: &str) -> Option<&'static str> {
    GUARDRAIL_POLICY_ENTRY_QUOTAS
        .iter()
        .find(|(path, _)| *path == member)
        .map(|(_, message)| *message)
}

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

/// What this service answers as its `ValidationException` message for a
/// `CreatePromptRouter` whose `promptRouterName` another router of this account and region
/// already holds.
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST: Amazon Bedrock reports that
/// the resource was not created and names the name the request asked for, so the rule is
/// "name the name the request carried" rather than one recorded router name. The name
/// "must be unique within your Amazon Web Services account in the current region"
/// (`smithy_query(query="doc",shape="CreatePromptRouterRequest",member="promptRouterName")`),
/// which is the rule this refusal reports.
#[must_use]
pub fn duplicate_prompt_router_message(prompt_router_name: &str) -> String {
    format!(
        "Failed to create the resource because the prompt router with name \
         {prompt_router_name} already exists."
    )
}

/// What this service answers as its `ValidationException` message for a delete addressing a
/// prompt router Amazon Bedrock itself publishes ([`PROMPT_ROUTER_TYPE_DEFAULT`]).
///
/// RECORDED behaviour, and a FIXED sentence: Amazon Bedrock reports that the caller is not
/// permitted to delete such a router and quotes no part of the request in it — not the ARN,
/// not the router's name — so the sentence has no variable part to fill in.
pub const DEFAULT_PROMPT_ROUTER_DELETE_REFUSED_MESSAGE: &str =
    "You don't have the permission to delete the default prompt router.";

/// What a request addressing a prompt router this account and region holds none of answers
/// as its `ResourceNotFoundException` message.
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST: Amazon Bedrock states that it
/// cannot find the router and names the `PromptRouterArn` the request carried, so the rule
/// is "name the ARN the request addressed" rather than one recorded ARN. Stated once here
/// because the message belongs to the RESOURCE rather than to one operation, the way
/// [`CUSTOM_MODEL_NOT_FOUND_MESSAGE`] is: a read and a delete of an ARN that resolves to no
/// router both report the same thing.
#[must_use]
pub fn prompt_router_not_found_message(prompt_router_arn: &str) -> String {
    format!("Cannot find prompt router {prompt_router_arn}.")
}

/// The most tags one resource of this service may carry.
///
/// `TooManyTagsException` documents the bound itself — "The request contains more tags
/// than can be associated with a resource (50 tags per resource)" — so a create carrying
/// more is refused with that error rather than stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// What this service answers as its `TooManyTagsException` message for a request carrying
/// more tags than one resource may hold.
///
/// RECORDED behaviour: Amazon Bedrock states the BOUND and nothing about the request — no
/// member path, no submitted tag count and no resource — so the one variable part is the
/// bound itself, read off [`MAX_TAGS_PER_RESOURCE`] rather than spelled into the sentence,
/// and every operation that refuses a request for its tag count answers alike.
#[must_use]
pub fn too_many_tags_message() -> String {
    format!("You can assign a maximum of {MAX_TAGS_PER_RESOURCE} tags to a resource")
}

/// What a tagging call whose `resourceARN` names no resource of this account answers as its
/// `ResourceNotFoundException` message.
///
/// RECORDED behaviour: Amazon Bedrock names the resource TYPE the value addressed and
/// nothing else — not the ARN the request carried, not the account — so the sentence tells a
/// caller WHICH kind of resource is missing, and a caller cannot tell one family's miss from
/// another's unless the type is in it. The type is the one the ARN itself spells, read
/// through `service_common::arn` — this crate's ONE ARN parse, which ends a type at either
/// separator AWS writes a resource with — and narrowed to this service's namespace, since an
/// ARN of another vendor names no resource of this API. (`TaggableResourcesArn` admits one of
/// this service's resource types there, or a bare resource NAME,
/// `smithy_query(query="shape",shape="TaggableResourcesArn")`.) The type is then read as the
/// noun phrase that opens the sentence: hyphens are spaces and only the first word is
/// capitalised. A value that spells no type of this service's namespace — a bare NAME, an
/// ARN of another vendor — names the resource generically.
#[must_use]
pub fn tagging_resource_not_found_message(resource_arn: &str) -> String {
    match ::service_common::arn::parse_of(resource_arn, ARN_NAMESPACE)
        .map(|arn| arn.resource_type())
    {
        Some(resource_type) if !resource_type.is_empty() => {
            format!("{} not found", resource_type_noun(resource_type))
        }
        _ => "Resource not found".to_string(),
    }
}

/// A resource type as the noun phrase that OPENS a sentence about it: the type spelled as
/// words ([`spelled_resource_type`]) with its first word capitalised
/// (`custom-model-deployment` is "Custom model deployment").
fn resource_type_noun(resource_type: &str) -> String {
    let spelled = spelled_resource_type(resource_type);
    let mut chars = spelled.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spelled,
    }
}

/// An ARN resource type spelled as WORDS: an ARN writes a type as a hyphenated token
/// (`custom-model`, `model-invocation-job`) and this service's sentences spell it with
/// spaces. ONE rendering, so the sentences that carry a resource type
/// ([`tagging_resource_not_found_message`], [`invalid_arn_format_message`]) cannot drift
/// apart.
fn spelled_resource_type(resource_type: &str) -> String {
    resource_type.replace('-', " ")
}

/// `MarketplaceModelEndpoint$status` — an endpoint Amazon Bedrock Marketplace knows.
///
/// The `Status` enum is `[REGISTERED, INCOMPATIBLE_ENDPOINT]`
/// (`smithy_query(query="shape",shape="Status")`) and the member documents it as "the
/// overall status of the endpoint in Amazon Bedrock Marketplace". An endpoint that exists
/// because `CreateMarketplaceModelEndpoint` was called was created by Bedrock from a
/// Marketplace model, so it is registered with Bedrock from that call onwards;
/// `INCOMPATIBLE_ENDPOINT` names an endpoint Bedrock cannot use, which no call here
/// produces. `RegisterMarketplaceModelEndpoint` is the other call that leaves an endpoint
/// in it: registering an endpoint with Bedrock Marketplace is what makes it one Bedrock
/// knows.
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

/// `ModelCopyJobStatus` — the copy Amazon Bedrock accepted is running.
///
/// The enum is `[InProgress, Completed, Failed]`
/// (`smithy_query(query="shape",shape="ModelCopyJobStatus")`). A copy runs after the call
/// that submits it returns, so this is the status every accepted job carries first.
pub const MODEL_COPY_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ModelCopyJobStatus` — the copy finished, which is what brings the copied custom model
/// into being; the terminal status the non-terminal one above is owed its move to.
///
/// `Failed` names a copy that did not finish; nothing in this emulator makes an accepted
/// copy fail, so no call here mints it.
pub const MODEL_COPY_JOB_STATUS_COMPLETED: &str = "Completed";

/// How long after `CreateModelCopyJob` a job's non-terminal `status` is owed its move to
/// [`MODEL_COPY_JOB_STATUS_COMPLETED`].
///
/// CHOSEN, not documented: AWS publishes no duration for copying a custom model into
/// another region. It is non-zero, so a caller that reads the job straight after creating
/// it still observes the `InProgress` status the real service reports; and it is
/// materialised from the stored due instant on the next read rather than waited on, so no
/// handler sleeps.
pub const MODEL_COPY_JOB_COMPLETE_AFTER: Duration = Duration::from_secs(3);

/// The most model copy jobs this account may hold at once, past which a create is refused
/// with 400 `ValidationException` — the refusal channel this operation's own errors do not
/// carry a quota error on.
///
/// The emulator's own bound rather than a published quota, for the reason
/// [`MAX_MODEL_IMPORT_JOBS_PER_ACCOUNT`] is one: without a server-side cap a caller could
/// grow this part of the store with nothing but fresh target model names.
pub const MAX_MODEL_COPY_JOBS_PER_ACCOUNT: usize = 100;

/// `EvaluationJobStatus` — the job Amazon Bedrock accepted is running.
///
/// The enum is `[InProgress, Completed, Failed, Stopping, Stopped, Deleting]`
/// (`smithy_query(query="shape",shape="EvaluationJobStatus")`). An accepted evaluation job
/// runs asynchronously, so this is the status it carries from the call that starts it.
///
/// It is a NON-TERMINAL status: the run it names ends, so the clock owes the job its move
/// to [`EVALUATION_JOB_STATUS_COMPLETED`] after [`EVALUATION_JOB_COMPLETE_AFTER`], the way
/// an accepted model import job is owed [`IMPORT_JOB_STATUS_COMPLETED`]. `Failed` names a
/// run that ended without its results; nothing here makes an accepted evaluation fail, so
/// no call mints it. `Stopping` and `Stopped` are reached by a caller's own
/// `StopEvaluationJob` (see [`EVALUATION_JOB_STATUS_STOPPING`]) and `Deleting` by
/// `BatchDeleteEvaluationJob`.
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

/// `EvaluationJobStatus` — the job ran to completion: the terminal status the non-terminal
/// [`EVALUATION_JOB_STATUS_IN_PROGRESS`] is owed its move to, after
/// [`EVALUATION_JOB_COMPLETE_AFTER`]. It is also one of the statuses a batch delete accepts
/// a job in.
pub const EVALUATION_JOB_STATUS_COMPLETED: &str = "Completed";

/// How long after `CreateEvaluationJob` an accepted job's non-terminal `status` is owed its
/// move to [`EVALUATION_JOB_STATUS_COMPLETED`].
///
/// CHOSEN, not documented: AWS publishes no duration for running an evaluation (the real
/// run depends on the dataset, the metrics and the models being evaluated), so this is the
/// emulator's span, the one [`MODEL_COPY_JOB_COMPLETE_AFTER`] uses for the same kind of
/// accepted-then-finishes job. It is non-zero, so a caller that reads the job straight
/// after creating it still observes the `InProgress` status the real service reports — and
/// can still stop it; and it is materialised from the stored due instant on the next call
/// that touches the record rather than waited on, so no handler sleeps.
pub const EVALUATION_JOB_COMPLETE_AFTER: Duration = Duration::from_secs(3);

/// `EvaluationJobStatus` — the job ended without producing its results. Nothing here makes
/// an accepted evaluation fail, so no call mints it; it is named because a job that carries
/// it is one a batch delete accepts.
pub const EVALUATION_JOB_STATUS_FAILED: &str = "Failed";

/// `EvaluationJobStatus` — the job a batch delete accepted is being deleted.
///
/// The status `BatchDeleteEvaluationJob` moves an accepted job to: deleting is
/// asynchronous (the operation answers the modelled 202), so this is the non-terminal
/// status the job carries from that call until the deletion has taken effect, after
/// [`EVALUATION_JOB_DELETED_AFTER`], at which point the job is gone and an identifier that
/// named it names nothing.
pub const EVALUATION_JOB_STATUS_DELETING: &str = "Deleting";

/// The statuses an evaluation job may be DELETED in: "An evaluation job can only be deleted
/// if it has following status FAILED, COMPLETED, and STOPPED"
/// (`smithy_query(query="operation",operation="BatchDeleteEvaluationJob")`), which is also
/// every TERMINAL value of the status enum.
pub const EVALUATION_JOB_DELETABLE_STATUSES: [&str; 3] = [
    EVALUATION_JOB_STATUS_COMPLETED,
    EVALUATION_JOB_STATUS_FAILED,
    EVALUATION_JOB_STATUS_STOPPED,
];

/// How long after `BatchDeleteEvaluationJob` an accepted job's `Deleting` status is owed its
/// move to being gone.
///
/// CHOSEN, not documented: AWS publishes no duration for deleting an evaluation job, so this
/// is the emulator's span, built the way [`EVALUATION_JOB_STOPPED_AFTER`] is. It is
/// non-zero, so a caller that reads a job straight after the delete still observes the
/// `Deleting` status the response reported; and it is materialised from the stored due
/// instant on the next call that touches the record rather than waited on, so no handler
/// sleeps.
pub const EVALUATION_JOB_DELETED_AFTER: Duration = Duration::from_millis(500);

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

/// `AdvancedPromptOptimizationJobStatus` — the job Amazon Bedrock accepted is running.
///
/// The enum is `[InProgress, Completed, Failed, PartiallyCompleted, Stopping, Stopped,
/// Deleting]` (`smithy_query(query="shape",shape="AdvancedPromptOptimizationJobStatus")`),
/// which is also what `crate::ops::types::ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_VALUES`
/// carries. The optimization runs after the create returns, so this is the status the job
/// carries from the call that starts it.
///
/// It is also the only status `CreateAdvancedPromptOptimizationJob` mints. `Completed`,
/// `PartiallyCompleted` and `Failed` mean the optimization ran over the caller's JSONL
/// input and its results were written to the request's `outputConfig` — work this emulator
/// does not perform, so no call here mints any of them and no clock transition leads to
/// them. `Stopping` and `Stopped` are reached by a caller's own
/// `StopAdvancedPromptOptimizationJob` (see
/// [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING`]) and `Deleting` by
/// `BatchDeleteAdvancedPromptOptimizationJob`.
pub const ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `AdvancedPromptOptimizationJobStatus` — the job is winding down because a caller
/// stopped it.
///
/// The status `StopAdvancedPromptOptimizationJob` moves a running job to: stopping a job is
/// asynchronous, so this is the non-terminal status the job carries from the call that asks
/// for the stop until the stop has taken effect. See
/// [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_IN_PROGRESS`] for the value list.
pub const ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING: &str = "Stopping";

/// `AdvancedPromptOptimizationJobStatus` once the stop has taken effect — the terminal
/// status the non-terminal [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING`] is owed its
/// move to, after [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STOPPED_AFTER`].
pub const ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopAdvancedPromptOptimizationJob` a job's `Stopping` status is owed its
/// move to [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`].
///
/// CHOSEN, not documented: AWS publishes no duration for stopping an advanced prompt
/// optimization job, so this is the emulator's span, the one
/// [`EVALUATION_JOB_STOPPED_AFTER`] uses for the same kind of wind-down. It is non-zero, so
/// a caller that reads the job straight after stopping it still observes the `Stopping`
/// status the real service reports; and it is materialised from the stored due instant on
/// the next call that touches the record rather than waited on, so no handler sleeps.
pub const ADVANCED_PROMPT_OPTIMIZATION_JOB_STOPPED_AFTER: Duration = Duration::from_millis(500);

/// The most advanced prompt optimization jobs this account may hold at once, past which a
/// create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota, for the reason
/// [`MAX_EVALUATION_JOBS_PER_ACCOUNT`] gives: without a server-side cap a caller could grow
/// this part of the store with nothing but fresh creates.
pub const MAX_ADVANCED_PROMPT_OPTIMIZATION_JOBS_PER_ACCOUNT: usize = 100;

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

/// What a request addressing a Provisioned Throughput this store does not hold answers as
/// its `ResourceNotFoundException` message, wherever that request came from: a read of it or
/// a delete of it.
///
/// RECORDED behaviour: Amazon Bedrock names the identifier the request provided and nothing
/// else — no account scope and no region — so the wording is fixed and the identifier is its
/// one variable part, echoed in the spelling the caller sent (`ProvisionedModelId` admits
/// both the name and the ARN). Stated once here because the message belongs to the RESOURCE
/// rather than to one operation, so every operation that addresses a Provisioned Throughput
/// refuses it the same way.
#[must_use]
pub fn provisioned_model_not_found_message(provisioned_model_id: &str) -> String {
    format!("Provisioned model with the provided name {provisioned_model_id} was not found.")
}

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
///
/// The remedy clause is [`service_limit_increase_case_url`] followed by the escalation it
/// asks for: a quota increase, which is the only way such a purchase ever succeeds.
#[must_use]
pub fn no_commitment_model_units_message(model_id: &str, region: &str) -> String {
    format!(
        "Your account does not currently have any no commitment model units reserved for \
         {model_id}. Please see {} to request a service quota increase.",
        service_limit_increase_case_url(region)
    )
}

/// The support-center case form a model-unit quota refusal hands the caller, for the
/// `region` the refused request addressed.
///
/// A Provisioned Throughput purchase refused for want of granted model units cannot be
/// retried into success — it can only be escalated — so the case form is the only
/// actionable content such a refusal carries, and it is the one the model documents for
/// `CreateProvisionedModelThroughputRequest.modelUnits`
/// (`smithy_query(query="doc",shape="CreateProvisionedModelThroughputRequest",member="modelUnits")`:
/// "You must first visit the ... support center to request MUs." over
/// `.../support/home#/case/create?issueType=service-limit-increase`). The
/// `issueType=service-limit-increase` selector is what preselects the quota-increase form,
/// so it is part of the link rather than decoration; the region the request addressed is
/// carried so the form opens against it.
#[must_use]
pub fn service_limit_increase_case_url(region: &str) -> String {
    format!(
        "https://support.console.aws.amazon.com/support/home?region={region}\
         #/case/create?issueType=service-limit-increase"
    )
}

/// What a Provisioned Throughput purchase answers as its `ValidationException` message when
/// the `modelId` names a model no Provisioned Throughput can be bought over.
///
/// RECORDED behaviour, and a FIXED sentence: it quotes no part of the request, so the model
/// the caller named is not echoed back. ONE sentence for every model that cannot back such a
/// purchase, because the reason is the same in each case — the model is offered under no
/// Provisioned Throughput: a model the catalog offers on demand only
/// ([`supports_provisioned_throughput`]) and a model this service recognises and offers no
/// longer ([`RETIRED_FOUNDATION_MODELS`]), which is offered under nothing at all.
pub const MODEL_NOT_SUPPORTED_FOR_PROVISIONED_THROUGHPUT: &str =
    "The provided model is not supported for provisioned throughput.";

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

/// `CustomModelDeploymentStatus` — the deployment is still being brought up by the create
/// call.
///
/// The enum is `[Creating, Active, Failed]`
/// (`smithy_query(query="shape",shape="CustomModelDeploymentStatus")`), which is also what
/// `crate::ops::types::CUSTOM_MODEL_DEPLOYMENT_STATUS_VALUES` carries.
pub const CUSTOM_MODEL_DEPLOYMENT_STATUS_CREATING: &str = "Creating";

/// `CustomModelDeploymentStatus` once the deployment serves on-demand inference — the
/// terminal status the non-terminal one above is owed its move to. See
/// [`CUSTOM_MODEL_DEPLOYMENT_STATUS_CREATING`] for the value list.
///
/// `Failed` names a deployment that could not be brought up; nothing implemented here
/// makes an accepted deployment fail, so it is never minted.
pub const CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE: &str = "Active";

/// How long after `CreateCustomModelDeployment` a deployment's non-terminal `status` is
/// owed its move to [`CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`].
///
/// CHOSEN, not documented: AWS publishes no duration for bringing a custom model
/// deployment up, so this is the emulator's span. It is non-zero, so a caller that reads
/// the deployment straight after creating it still observes the `Creating` status the real
/// service reports; and it is materialised from the stored due instant on the next call
/// that touches the record rather than waited on, so no handler sleeps.
pub const CUSTOM_MODEL_DEPLOYMENT_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most custom model deployments this account may hold at once, across every region,
/// past which a create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota, and account-wide because what it
/// bounds is the STORE: without a server-side cap a caller could grow this part of it with
/// nothing but fresh deployment names, in one region or in many.
pub const MAX_CUSTOM_MODEL_DEPLOYMENTS_PER_ACCOUNT: usize = 100;

// The base-model segment a `CustomModelArn` carries for a model that was IMPORTED rather
// than customized from a base model is `CUSTOM_MODEL_IMPORTED_BASE`, which the custom
// model family below already states: `CustomModelArn` spells the segment
// `custom-model/(imported|<base model id>)/<id>`
// (`smithy_query(query="operation",operation="CreateCustomModelDeployment")`), so that one
// literal is the alternative to a base model id and names no foundation model for the
// published catalog to carry — a deployment create reads it from there rather than
// declaring a second spelling of the same token.

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

/// What a request addressing a custom model this store cannot resolve answers as its
/// `ResourceNotFoundException` message.
///
/// RECORDED behaviour: Amazon Bedrock states one FIXED sentence for that class and quotes
/// no part of the request in it — not the identifier, not the account and not the region —
/// so the sentence has no variable part to fill in. Stated once here because the message
/// belongs to the RESOURCE rather than to one operation, exactly as
/// [`provisioned_model_not_found_message`] is, so every operation that addresses a custom
/// model refuses an identifier it cannot resolve to one the same way.
pub const CUSTOM_MODEL_NOT_FOUND_MESSAGE: &str = "Custom model not found.";

/// The one sentence Amazon Bedrock states for a model identifier it cannot resolve to a
/// model at all: RECORDED behaviour, and a FIXED sentence — it quotes no part of the
/// request, so there is nothing of the caller's value in it.
///
/// Taken from the Bedrock Runtime emulator, which states the same refusal for the same
/// class on the inference surface, rather than spelled a second time here: one wording for
/// one sentence.
pub const INVALID_MODEL_IDENTIFIER_MESSAGE: &str =
    ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::INVALID_MODEL_IDENTIFIER_MESSAGE;

/// How a `ModelIdentifier` that resolves to no custom model of this account and Region is
/// refused — the ONE reading of that failure every operation addressing a custom model by
/// that member shares ([`unresolved_custom_model`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedCustomModel {
    /// The value is a `custom-model` ARN of this service's namespace that names no stored
    /// model, so it is an ARN to report missing: `ResourceNotFoundException` (404), which is
    /// what that error documents itself as ("The specified resource Amazon Resource Name
    /// (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request
    /// again"), carrying [`CUSTOM_MODEL_NOT_FOUND_MESSAGE`].
    MissingArn,
    /// The value is one of the member's other spellings — a bare model NAME — that names no
    /// model, so there is no ARN to report missing: it is an identifier this service could
    /// not resolve, which is `ValidationException` (400) carrying
    /// [`INVALID_MODEL_IDENTIFIER_MESSAGE`].
    UnresolvableIdentifier,
}

impl UnresolvedCustomModel {
    /// The sentence this refusal carries. Fixed text in both readings: neither quotes the
    /// identifier the request addressed, so the two spellings of one missing model are told
    /// apart by the error, never by a value echoed back.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingArn => CUSTOM_MODEL_NOT_FOUND_MESSAGE,
            Self::UnresolvableIdentifier => INVALID_MODEL_IDENTIFIER_MESSAGE,
        }
    }
}

/// How `identifier` — a `ModelIdentifier` that [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::custom_model`]
/// resolved to nothing — is refused.
///
/// The member's `@pattern` admits the model's ARN **or** its bare name
/// (`smithy_query(query="operation",operation="GetCustomModel,DeleteCustomModel")`), and the
/// two unresolvable spellings are not the same failure, so which one the caller sent is read
/// off the value through this crate's one ARN parse ([`names_arn_of`]). Stated once because
/// the reading belongs to the RESOURCE rather than to one operation: a read and a delete of
/// an identifier that resolves to no custom model refuse it alike.
#[must_use]
pub fn unresolved_custom_model(identifier: &str) -> UnresolvedCustomModel {
    if names_arn_of(identifier, CUSTOM_MODEL_RESOURCE_TYPE) {
        UnresolvedCustomModel::MissingArn
    } else {
        UnresolvedCustomModel::UnresolvableIdentifier
    }
}

/// What a create whose artifacts are named by a `modelSourceConfig` answers as its
/// `ValidationException` message when the request names no role Amazon Bedrock can read
/// those artifacts as.
///
/// RECORDED behaviour: the role is what Amazon Bedrock reads the Amazon S3 artifacts as, so
/// a request that supplies none usable is refused ON THE ROLE — one FIXED sentence saying
/// the role ARN provided is invalid, quoting neither the member name nor any value — rather
/// than as a missing member, which would point a caller at the shape of its request instead
/// of at the role it has to supply.
pub const INVALID_ROLE_ARN_MESSAGE: &str = "The provided role ARN is invalid.";

/// `ModelCustomizationJobStatus` — the customization job Amazon Bedrock accepted is
/// training.
///
/// The enum is `[InProgress, Completed, Failed, Stopping, Stopped]`
/// (`crate::ops::types::MODEL_CUSTOMIZATION_JOB_STATUS_VALUES`), and
/// `CreateModelCustomizationJob` documents that "model-customization jobs are asynchronous
/// and the completion time depends on the base model and the training/validation data
/// size" (`smithy_query(query="doc",shape="CreateModelCustomizationJob")`) — so this is
/// the status a job carries from the call that starts it.
pub const MODEL_CUSTOMIZATION_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ModelCustomizationJobStatus` once the customization has finished — the terminal status
/// the one above is owed its move to.
///
/// "After the model-customization job completes successfully, your custom model resource
/// will be ready to use" (`smithy_query(query="doc",shape="CreateModelCustomizationJob")`),
/// so reaching this status is what brings the job's output model into being.
/// `Failed` names a job whose customization did not finish; nothing here makes an accepted
/// job fail, so no call mints it.
pub const MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED: &str = "Completed";

/// `ModelCustomizationJobStatus` — the job is winding down because a caller stopped it.
///
/// The status `StopModelCustomizationJob` moves a running job to: stopping a job is
/// asynchronous, so this is the non-terminal status the job carries from the call that asks
/// for the stop until the stop has taken effect.
pub const MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING: &str = "Stopping";

/// `ModelCustomizationJobStatus` once the stop has taken effect — the terminal status the
/// non-terminal one above is owed its move to, after
/// [`MODEL_CUSTOMIZATION_JOB_STOPPED_AFTER`].
pub const MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED: &str = "Stopped";

/// How long after `CreateModelCustomizationJob` a job's non-terminal `status` is owed its
/// move to [`MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED`].
///
/// CHOSEN, not documented: the model says the completion time "depends on the base model
/// and the training/validation data size", so AWS publishes no duration and this is the
/// emulator's span. It is non-zero, so a caller that reads or stops the job straight after
/// creating it still observes the `InProgress` status the real service reports; and it is
/// materialised from the stored due instant on the next call that touches the record rather
/// than waited on, so no handler sleeps.
pub const MODEL_CUSTOMIZATION_JOB_COMPLETE_AFTER: Duration = Duration::from_millis(500);

/// How long after `StopModelCustomizationJob` a job's `Stopping` status is owed its move to
/// [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED`].
///
/// CHOSEN, not documented: AWS publishes no duration for stopping a customization job (the
/// real stop has to wind down training that is already running), so this is the emulator's
/// span — materialised from the stored due instant, exactly as the completion one is.
pub const MODEL_CUSTOMIZATION_JOB_STOPPED_AFTER: Duration = Duration::from_millis(500);

/// `CustomizationType` — tuning a base model on labelled data, and the customization a
/// create that names none runs.
///
/// `CreateModelCustomizationJob` "creates a FINE-TUNING job to customize a base model"
/// (`smithy_query(query="doc",shape="CreateModelCustomizationJob")`), while
/// `customizationType` is optional and the model declares no default for it — so a request
/// that names none asks for the job that operation is documented to create. See
/// [`CUSTOMIZATION_TYPE_IMPORTED`] for the value list.
pub const CUSTOMIZATION_TYPE_FINE_TUNING: &str = "FINE_TUNING";

/// The most model customization jobs this account may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota (AWS publishes a quota for
/// CONCURRENT customization jobs, not for the jobs an account's history holds): without a
/// server-side cap a caller could grow this part of the store with nothing but fresh job
/// names.
pub const MAX_MODEL_CUSTOMIZATION_JOBS_PER_ACCOUNT: usize = 100;

/// `InferenceProfileStatus` — the only status an inference profile reports.
///
/// The enum is `[ACTIVE]` (`smithy_query(query="shape",shape="InferenceProfileStatus")`),
/// documented as "`ACTIVE` means that the inference profile is ready to be used", so a
/// created profile is READY and is owed no later transition.
pub const INFERENCE_PROFILE_ACTIVE: &str = "ACTIVE";

/// `InferenceProfileType` — the type of a profile a caller created.
///
/// The enum is `[SYSTEM_DEFINED, APPLICATION]`, and `APPLICATION` is documented as "the
/// inference profile was created by a user"
/// (`smithy_query(query="shape",shape="InferenceProfileSummary")`), which is every profile
/// this store holds: `SYSTEM_DEFINED` profiles are the ones Amazon Bedrock itself defines
/// ([`system_defined_inference_profiles`]), and no operation of this service creates one.
pub const INFERENCE_PROFILE_TYPE_APPLICATION: &str = "APPLICATION";

/// What Amazon Bedrock states when an `inferenceProfileIdentifier` names no inference
/// profile: the message every 404 `ResourceNotFoundException` of this resource carries.
///
/// Stated once here because the read and the delete answer the same refusal for the same
/// identifier, and two spellings of it could drift. It names no identifier: the wording is
/// the error class's, with nothing of the request in it.
pub const INFERENCE_PROFILE_NOT_FOUND: &str = "Inference profile not found.";

/// The most application inference profiles this account may hold at once, across every
/// region it has created one in, past which a create is refused with
/// `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota: without a server-side cap a
/// caller could grow this part of the store with nothing but fresh profile names, or with
/// fresh regions, which is why the bound is over the whole store rather than per region.
pub const MAX_INFERENCE_PROFILES_PER_ACCOUNT: usize = 100;

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

    /// Remove version `number` and release the idempotency tokens that named it, through
    /// the one rule [`drop_version`] states for both versioned resources of this service.
    pub fn remove_version(&mut self, number: u32) -> Option<GuardrailRevision> {
        drop_version(&mut self.versions, &mut self.version_tokens, number)
    }
}

/// Remove version `number` from `versions` and release the entries of `tokens` that named
/// it, handing back what was removed.
///
/// Here rather than on each record because this service has TWO versioned resources — a
/// guardrail and an Automated Reasoning policy — and each holds its snapshots and the
/// idempotency tokens that cut them in this same pair of maps, so dropping one version is
/// ONE rule: a token never outlives the version it minted.
fn drop_version<V>(
    versions: &mut BTreeMap<u32, V>,
    tokens: &mut HashMap<String, u32>,
    number: u32,
) -> Option<V> {
    let removed = versions.remove(&number)?;
    tokens.retain(|_, minted| *minted != number);
    Some(removed)
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

/// One stored application inference profile: the identity `CreateInferenceProfile`
/// minted, the model source that request named, and what a read reports.
///
/// `status` and `profile_type` are `&'static str` for the same reason
/// [`PromptRouter::status`] is: both are single values this emulator mints
/// ([`INFERENCE_PROFILE_ACTIVE`], [`INFERENCE_PROFILE_TYPE_APPLICATION`]) rather than
/// anything a request chooses.
#[derive(Debug, Clone)]
pub struct InferenceProfileRecord {
    /// The `InferenceProfileArn` this profile is addressed by, minted by
    /// [`application_inference_profile_arn`].
    pub arn: String,
    /// `inferenceProfileId` — the resource half of that ARN, so both spellings of an
    /// `InferenceProfileIdentifier` name this one record.
    pub id: String,
    /// `inferenceProfileName`, as the create request spelled it.
    pub name: String,
    pub description: Option<String>,
    /// The region the ARN carries, so a profile is only ever read in the region it was
    /// created in.
    pub region: String,
    /// `models` — the foundation models this profile tracks metrics and costs for,
    /// derived from the `modelSource` the create named.
    pub models: Vec<InferenceProfileModel>,
    pub tags: Vec<Tag>,
    /// `InferenceProfileStatus`; [`INFERENCE_PROFILE_ACTIVE`] for every profile minted
    /// here.
    pub status: &'static str,
    /// `InferenceProfileType`; [`INFERENCE_PROFILE_TYPE_APPLICATION`] for a profile a
    /// caller created.
    pub profile_type: &'static str,
    pub created_at: SystemTime,
    /// When the profile last changed. Equal to [`InferenceProfileRecord::created_at`]
    /// until some operation changes it; the model carries no `UpdateInferenceProfile`, so
    /// no request moves it today.
    pub updated_at: SystemTime,
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

/// `DataRetentionMode` — the account-wide mode in force while no put has set one.
///
/// The model documents this value as "The standard data handling for the model applies"
/// (`smithy_query(query="shape",shape="DataRetentionMode")`), which is the handling an
/// account that has never configured a mode gets. `inherit` is documented as "No data
/// retention mode is set at this scope", so it is a mode a caller ASKS for rather than the
/// standing answer for an account that asked for nothing.
pub const DATA_RETENTION_MODE_DEFAULT: &str = "default";

/// What `GetUseCaseForModelAccess` answers as its `ResourceNotFoundException` message while
/// the account has submitted no form
/// ([`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`]).
///
/// RECORDED behaviour: Amazon Bedrock tells the caller what to DO — fill the form out before
/// asking for access — in a wholly fixed sentence that carries nothing about the account, so
/// a wrapper surfacing the message verbatim shows the same guidance production does.
pub const USE_CASE_FOR_MODEL_ACCESS_NOT_SUBMITTED: &str =
    "You have not filled out the request form. Fill out the form before getting access.";

/// The account's data retention setting: the `mode` `PutAccountDataRetention` last stored,
/// and the instant that put stored it.
///
/// ONE record per account rather than a map, for the reason
/// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`]
/// is one: the operations that write and read it carry no identifier at all, so there is a
/// single account-level setting rather than a resource a caller can hold more than one of,
/// and a further put REPLACES this record whole.
#[derive(Debug, Clone)]
pub struct AccountDataRetention {
    /// `mode` — one of `DataRetentionMode`'s values, as the admitted put carried it. Any
    /// other value is refused by the constraint walk before the handler runs, so what is
    /// stored here is always a modelled mode.
    pub mode: String,
    /// `updatedAt` — when the last admitted put stored this mode.
    pub updated_at: SystemTime,
}

/// What this control plane answers when a PUT addresses an enforced guardrail configuration
/// this account does not hold: the `configId` names no configuration, which is the 404
/// `ResourceNotFoundException` that put answers.
///
/// The emulator's own wording, and NOT the sentence the delete of a configuration answers
/// ([`ENFORCED_GUARDRAIL_CONFIGURATION_NOT_FOUND`]), which is recorded: the two refusals of
/// this resource are worded independently, each from its own evidence, so the sentence one
/// answers is no evidence about the other.
#[must_use]
pub fn enforced_guardrail_configuration_not_found(config_id: &str) -> String {
    format!("no enforced guardrail configuration {config_id} exists in this account")
}

/// What `DeleteEnforcedGuardrailConfiguration` answers when the `configId` it carries names
/// no configuration this account holds.
///
/// RECORDED behaviour, and a message with NO variable parts: Amazon Bedrock reports that no
/// configuration was found for the identifier the request carried without repeating that
/// identifier, so the wording is the whole requirement.
pub const ENFORCED_GUARDRAIL_CONFIGURATION_NOT_FOUND: &str =
    "No default guardrail configuration found for the specified config identifier.";

/// Put the account's enforced guardrail configuration IN FORCE on the inference plane:
/// `config`, enforcing `revision`, becomes the guardrail every covered invocation of this
/// account is evaluated against. `None` withdraws whatever stood before it.
///
/// Answers whether the inference plane's store was reached: an emulator assembled without
/// the Bedrock Runtime data plane enforces nothing, and this control plane's own record of
/// the configuration is unaffected either way.
///
/// Call it with NO store of this service held, so two services' stores are never locked at
/// once.
pub fn enforce_guardrail_on_inference<Eng: Send + Sync + 'static>(
    engine: &Eng,
    installed: Option<(&EnforcedGuardrailConfiguration, &GuardrailRevision)>,
) -> bool {
    runtime_guardrail::install(
        engine,
        installed.map(|(config, revision)| enforced_guardrail_enforcement(config, revision)),
    )
}

/// The ENFORCEMENT an installed configuration is, as the inference plane applies it.
///
/// The configuration this control plane stores is the account's resource — its `configId`,
/// the principals and the instants a listing reports. What the INFERENCE plane needs is
/// something else: the policies to evaluate a request against. The two planes keep one
/// copy of that enforcement, in the inference plane's own store
/// (`PutEnforcedGuardrailConfiguration` installs it there through
/// [`enforce_guardrail_on_inference`]),
/// and this is the projection that fills it: the enforced revision's word policy and
/// blocked messaging, together with the model enforcement and selective-guarding modes the
/// put carried.
///
/// `revision` is the guardrail revision the configuration enforces — a NUMBERED version,
/// which is an immutable snapshot of the guardrail's policies, so the enforcement installed
/// from it stays the revision inference evaluates for as long as the configuration stands.
fn enforced_guardrail_enforcement(
    config: &EnforcedGuardrailConfiguration,
    revision: &GuardrailRevision,
) -> runtime_guardrail::EnforcedGuardrail {
    let word_policy = revision.word_policy.as_ref();
    runtime_guardrail::EnforcedGuardrail {
        guardrail_id: config.guardrail_id.clone(),
        guardrail_version: config.guardrail_version.clone(),
        blocked_input_messaging: revision.blocked_input_messaging.clone(),
        blocked_outputs_messaging: revision.blocked_outputs_messaging.clone(),
        words: word_policy
            .and_then(|policy| policy.words.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|word| {
                runtime_guardrail::BlockedWord::new(
                    word.text.clone(),
                    word.input_action.as_deref(),
                    word.input_enabled,
                    word.output_action.as_deref(),
                    word.output_enabled,
                )
            })
            .collect(),
        managed_word_lists: word_policy
            .and_then(|policy| policy.managed_word_lists.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|list| {
                runtime_guardrail::ManagedWordList::new(
                    list.r#type.clone(),
                    list.input_action.as_deref(),
                    list.input_enabled,
                    list.output_action.as_deref(),
                    list.output_enabled,
                )
            })
            .collect(),
        // A configuration carrying no `modelEnforcement` is enforced on every model, which
        // the inference plane reads from an EMPTY included list; one that carries it names
        // the models to enforce on and the ones to exclude.
        included_models: config
            .model_enforcement
            .as_ref()
            .map(|enforcement| enforcement.included_models.clone())
            .unwrap_or_default(),
        excluded_models: config
            .model_enforcement
            .as_ref()
            .map(|enforcement| enforcement.excluded_models.clone())
            .unwrap_or_default(),
        system_guarded_selectively: config
            .selective_content_guarding
            .as_ref()
            .and_then(|guarding| guarding.system.as_deref())
            .is_some_and(|mode| mode == runtime_guardrail::GUARDING_SELECTIVE),
        messages_guarded_selectively: config
            .selective_content_guarding
            .as_ref()
            .and_then(|guarding| guarding.messages.as_deref())
            .is_some_and(|mode| mode == runtime_guardrail::GUARDING_SELECTIVE),
    }
}

/// Amazon Bedrock's answer when it is asked to take on a role the caller may not hand it:
/// the `iam:PassRole` the request would make is one this account does not hold.
///
/// The one sentence, with no variable part: the denial names neither the role nor the
/// destination, so there is nothing of the request to fill in. It is spelled the way the
/// rest of this tree records AWS spelling the same denial. It is answered with the modelled
/// `AccessDeniedException` (`@httpError(403)`).
pub const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";

/// Why Amazon Bedrock may not be passed the role a model invocation logging `config` names,
/// or `None` when every role it names may be passed — which is the ordinary case.
///
/// A destination that names a role the CALLER supplied needs an `iam:PassRole` step before
/// any log event can be written, and the only such destination is CloudWatch Logs: its
/// `roleArn` is `@required` and is the role Bedrock must take on to write into the log
/// group the same structure names
/// (`smithy_query(query="shape",shape="CloudWatchConfig")`). `S3Config` declares no role
/// member at all, so an S3-only configuration — and one naming no destination — passes no
/// role, has no `iam:PassRole` step to authorize, and is admitted; whether the account may
/// write to a named bucket is not a decision about a passed role.
///
/// The decision for the CloudWatch destination is taken over the SUBMITTED value, in two
/// parts, and a pass that neither part refuses is made:
///
/// * the ACCOUNT the value names. `iam:PassRole` is granted by the account that HOLDS the
///   role, so a principal of this account can hand Amazon Bedrock a role of its own account
///   and no role of another one, whatever that other role trusts. The account is read out of
///   the ARN by the crate's one IAM-ARN reading
///   ([`::service_common::iam_role::role_name`]), which answers the role's name only for a
///   role of THIS account and treats the EMPTY account field the member's published pattern
///   admits (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`) as this account, since a value
///   naming no account names no other one. The pattern itself is refused by the constraint
///   walk ahead of this gate, so a value that reaches here and reads as no role of this
///   account is one whose embedded 12-digit account is another's;
/// * the TRUST the role of this account carries. Amazon Bedrock takes the role on to write
///   into the log group beside it, which it can do only where that role's trust policy
///   admits [`BEDROCK_SERVICE_PRINCIPAL`] the `sts:AssumeRole` such a hand-off is
///   ([`::service_common::iam_role::role_is_unassumable`], the tree's one reading of a
///   caller-supplied role against this account's IAM). This call STORES a trust
///   relationship for every later invocation to be logged under rather than authorizing one
///   named assume, so the trust is validated as a relationship: a statement's `Condition`
///   narrows which assumes it covers, not whether the role trusts the service, and the
///   confused-deputy clauses AWS documents on a logging role are over keys a single
///   invocation carries rather than values of this request.
///
/// Refused only on what this account's IAM ANSWERED about the role — it holds no such role,
/// or the trust document it reported withholds the grant. An emulator with no IAM to ask,
/// or an answer that could not be read, leaves the pass admitted, because a refusal has to
/// be grounded.
///
/// A denied pass is [`CROSS_ACCOUNT_PASS_ROLE_DENIED`] in the modelled
/// `AccessDeniedException` (`@httpError(403)`) with nothing installed; a pass this account
/// may make installs the configuration, so the CloudWatch Logs destination is reported back
/// by the get, cleared by the delete, and honoured where the configuration is enforced.
pub async fn pass_role_denial(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &LoggingConfig,
) -> Option<String> {
    let role_arn = &config.cloud_watch_config.as_ref()?.role_arn;
    if ::service_common::iam_role::role_name(role_arn).is_none() {
        return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string());
    }
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        role_arn,
        BEDROCK_SERVICE_PRINCIPAL,
    )
    .await
    .then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string())
}

/// Why Amazon Bedrock may not be passed the role a JOB-creating request names, or `None`
/// when it may be passed.
///
/// Starting a job makes Amazon Bedrock TAKE ON the submitted role — to read the job's
/// inputs and to write the artifacts it produces — so `roleArn` names a role the service
/// assumes rather than a string the request stores, and the `iam:PassRole` step that
/// decides whether this caller may pass it stands in front of everything else the request
/// needs: no resource it names is resolved until that decision is taken. The operations
/// that make such a pass are the job creates whose `roleArn` is the role the job RUNS as —
/// `CreateModelCustomizationJob`, `CreateModelInvocationJob` and `CreateEvaluationJob`
/// (`smithy_query(query="operation",operation="CreateModelCustomizationJob,CreateModelInvocationJob,CreateEvaluationJob")`:
/// each declares `roleArn` `@required` under `RoleArn` and `AccessDeniedException` at
/// `@httpError(403)`) — and each takes this one decision rather than a copy of it.
///
/// The decision is the one every service that is handed a role to act under makes
/// ([`::service_common::iam_role::role_pass_is_unauthorized`]), asked for THIS pass:
///
/// * the value has to name a role of the CALLER's OWN account — `iam:PassRole` is granted by
///   the account that holds the role, so another account's role is one no principal of this
///   account can hand to a service, whatever that role trusts (an EMPTY account field, which
///   the member's published pattern admits,
///   `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, names no other account, hence this one);
/// * this account's IAM has to HOLD that role; and
/// * the trust policy the role carries has to grant [`BEDROCK_SERVICE_PRINCIPAL`] the
///   `sts:AssumeRole` this pass would make. A statement's `Condition` is part of its grant,
///   so a grant this pass cannot be shown to satisfy is not one: the pass carries the
///   account whose request Amazon Bedrock would be acting for
///   ([`SOURCE_ACCOUNT_CONDITION_KEY`]) and nothing else — the job the role would be assumed
///   for does not exist while its own creation is still being authorized, so its ARN is no
///   value of this request — and a clause over any other key leaves the statement undecided,
///   which IAM never reads as a grant. RECORDED, not derived from the documentation: the
///   corpus records this operation REFUSING a pass of exactly that shape — a role of the
///   caller's own account, trusted to this service, whose grant is narrowed to the ARN of
///   the very job the call would create — and no value is guessed for a key the request does
///   not carry (`::service_common::iam_authorization` decides a clause or leaves it
///   undecided; it never fills one in).
///
/// Refused only on what IAM ANSWERED: an emulator with no IAM to ask, or an answer this
/// could not read, leaves the pass admitted, because a refusal has to be grounded.
///
/// ONE sentence for every refusal this gate makes, [`CROSS_ACCOUNT_PASS_ROLE_DENIED`]: the
/// corpus records these operations refusing a pass with that sentence and with nothing else,
/// and it records it for a role of the caller's OWN account — so the recorded wording is not
/// reserved for the ARN naming another account, and no recording of them words a
/// refused pass any other way. Inventing a second sentence for the other paths of one
/// decision would be a wording no recording supports; so the gate states the recorded one,
/// which carries no part of the request to be wrong about.
///
/// A denied pass is the modelled `AccessDeniedException` (`@httpError(403)`) carrying
/// [`CROSS_ACCOUNT_PASS_ROLE_DENIED`], with no job and nothing else written, which is
/// what every recording of such a create shows.
pub async fn job_pass_role_denial(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<String> {
    let context = [(
        SOURCE_ACCOUNT_CONDITION_KEY.to_string(),
        ::engine::emulator_account_id().to_string(),
    )];
    ::service_common::iam_role::role_pass_is_unauthorized(
        ctx,
        region,
        role_arn,
        BEDROCK_SERVICE_PRINCIPAL,
        &context,
    )
    .await
    .then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string())
}

/// The principal Amazon Bedrock takes on a role a caller passes it AS.
///
/// This service's `aws.api#service$cloudTrailEventSource`
/// (`smithy_query(query="service_info")`), which is the service principal AWS names Amazon
/// Bedrock by in an IAM policy — the principal a role that Amazon Bedrock is to assume has
/// to trust.
pub const BEDROCK_SERVICE_PRINCIPAL: &str = "bedrock.amazonaws.com";

/// `aws:SourceAccount` — IAM's global condition key for the account whose request the
/// service is acting for, which is the one value an `iam:PassRole` decision of this account
/// carries: the account making the call is the account the service would act for.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

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
/// * `advanced_prompt_optimization_jobs` holds one entry per stored advanced prompt
///   optimization job, capped at
///   [`MAX_ADVANCED_PROMPT_OPTIMIZATION_JOBS_PER_ACCOUNT`] by the quota
///   `CreateAdvancedPromptOptimizationJob` enforces (the entry is keyed by the job ARN,
///   which carries a freshly minted id, so no two creates can address one entry), and
///   `advanced_prompt_optimization_job_create_tokens` holds at most one entry per stored
///   job for the same reason `create_tokens` does.
///
/// * `marketplace_model_endpoints` holds one entry per live marketplace model endpoint,
///   capped at [`MAX_MARKETPLACE_MODEL_ENDPOINTS_PER_ACCOUNT`] by the quota
///   `CreateMarketplaceModelEndpoint` enforces (the entry is keyed by the endpoint ARN,
///   which carries the endpoint name, so two creates of one name in one region cannot add
///   two entries), `marketplace_endpoint_create_tokens` holds at most one entry per live
///   endpoint for the same reason `create_tokens` does — including across a
///   deregistration, which drops the entries naming the endpoint it removed — and each
///   endpoint's
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
/// * `custom_model_deployments` and `custom_model_deployment_names` hold one entry per
///   live custom model deployment, capped at
///   [`MAX_CUSTOM_MODEL_DEPLOYMENTS_PER_ACCOUNT`] by the quota
///   `CreateCustomModelDeployment` enforces, and
///   `custom_model_deployment_create_tokens` holds at most one entry per live deployment
///   for the same reason `create_tokens` does.
///
/// * `model_invocation_jobs` holds one entry per submitted batch inference job, capped at
///   [`MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT`] by the quota `CreateModelInvocationJob`
///   enforces (the entry is keyed by the job ARN, which carries a freshly minted id, so no
///   two submissions can address one entry), and `model_invocation_job_create_tokens`
///   holds at most one entry per stored job for the same reason `create_tokens` does.
///
/// * `automated_reasoning_policies` and `automated_reasoning_policy_names` hold one entry
///   per live Automated Reasoning policy, capped at
///   [`MAX_AUTOMATED_REASONING_POLICIES_PER_ACCOUNT`] by the quota
///   `CreateAutomatedReasoningPolicy` enforces, and
///   `automated_reasoning_policy_create_tokens` holds at most one entry per live policy for
///   the same reason `create_tokens` does.
///
/// * each policy's [`AutomatedReasoningPolicyRecord::versions`] holds one entry per version
///   cut from it, capped at [`MAX_AUTOMATED_REASONING_POLICY_VERSIONS_PER_POLICY`] by the
///   quota `CreateAutomatedReasoningPolicyVersion` enforces, and that policy's
///   [`AutomatedReasoningPolicyRecord::version_tokens`] is bounded the same way against
///   those versions.
///
/// * `automated_reasoning_policy_build_workflows` holds one entry per started build
///   workflow, capped at [`MAX_BUILD_WORKFLOWS_PER_POLICY`] per policy by the quota
///   `StartAutomatedReasoningPolicyBuildWorkflow` enforces — and so at that bound times
///   [`MAX_AUTOMATED_REASONING_POLICIES_PER_ACCOUNT`] over the whole store — while
///   `automated_reasoning_policy_build_workflow_tokens` holds at most one entry per
///   started workflow for the same reason `create_tokens` does. No entry holds a source
///   document's bytes, only the metadata a read reports.
///
/// * `automated_reasoning_policy_test_cases` holds one entry per stored test, capped per
///   policy at [`MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY`] by the quota
///   `CreateAutomatedReasoningPolicyTestCase` enforces — and the policies those tests hang
///   off are themselves capped — and
///   `automated_reasoning_policy_test_case_create_tokens` holds at most one entry per
///   stored test for the same reason `create_tokens` does.
///
/// * `automated_reasoning_policy_test_runs` holds one entry per `(policy, build workflow,
///   test)` triple a test workflow has scheduled, so it is bounded by the product of the
///   tests and the build workflows those two quotas already admit — a test workflow re-run
///   replaces the entry it scheduled before rather than adding one — while
///   `automated_reasoning_policy_test_workflow_tokens` holds at most ONE entry per build
///   workflow, which is bounded by that same build workflow quota.
///
/// * `inference_profiles` holds one entry per live application inference profile, capped
///   at [`MAX_INFERENCE_PROFILES_PER_ACCOUNT`] by the quota `CreateInferenceProfile`
///   enforces (the entry is keyed by the profile ARN, which carries the minted id, so no
///   two creates can address one entry), and `inference_profile_create_tokens` holds at
///   most one entry per live profile for the same reason `create_tokens` does — including
///   across a delete, which drops the entries naming the profile it removed.
///
/// * `foundation_models` is the PUBLISHED catalog, seeded once per store and never
///   written: no operation here creates or deletes a foundation model.
///
/// * `resource_policies` holds at most one entry per live resource of the maps above: a
///   policy is only ever attached to a resource this store HOLDS, and both the attach and
///   the detach drop the entries whose resource is gone
///   ([`Self::attach_resource_policy`], [`Self::detach_resource_policy`]).
///
/// * `use_case_for_model_access` is a single optional form, which a put replaces rather
///   than appends to, and whose size the model already caps
///   (`AcknowledgementFormDataBody` is 10..16384 bytes).
///
/// * `account_data_retention` is a single optional setting, which a put replaces rather
///   than appends to, and whose one member is a value out of a fixed enum.
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
    /// Every Bedrock Marketplace model endpoint this account has REGISTERED with Amazon
    /// Bedrock, keyed by its `endpointArn` — the one identifier
    /// `GetMarketplaceModelEndpoint`, `UpdateMarketplaceModelEndpoint`,
    /// `RegisterMarketplaceModelEndpoint`, `DeregisterMarketplaceModelEndpoint` and
    /// `DeleteMarketplaceModelEndpoint` address an endpoint by.
    ///
    /// Membership here IS the registration: an endpoint
    /// `DeregisterMarketplaceModelEndpoint` removed is no longer one Amazon Bedrock knows,
    /// whatever became of the SageMaker endpoint behind it, which this emulator holds no
    /// state for.
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
    /// Every stored advanced prompt optimization job, keyed by its
    /// `AdvancedPromptOptimizationJobArn` — one of the two spellings an
    /// `AdvancedPromptOptimizationJobIdentifier` carries, the other being the minted job
    /// id, which that ARN itself carries
    /// (`smithy_query(query="operation",operation="GetAdvancedPromptOptimizationJob")`).
    pub advanced_prompt_optimization_jobs: HashMap<String, AdvancedPromptOptimizationJobRecord>,
    /// `CreateAdvancedPromptOptimizationJob` idempotency: `(region, clientToken)` -> the
    /// `jobArn` that token minted. Scoped to that one operation, and to the region the
    /// request addressed, the way the evaluation job index is.
    pub advanced_prompt_optimization_job_create_tokens: HashMap<(String, String), String>,
    /// How many advanced prompt optimization job ids this store has minted; the uniqueness
    /// half of a minted id.
    pub minted_advanced_prompt_optimization_jobs: u64,
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
    /// Every stored custom model deployment, keyed by its `customModelDeploymentArn`.
    pub custom_model_deployments: HashMap<String, CustomModelDeploymentRecord>,
    /// `(region, modelDeploymentName)` -> `customModelDeploymentArn`, because the name is
    /// the other spelling of a `CustomModelDeploymentIdentifier` and is unique within an
    /// account and region.
    pub custom_model_deployment_names: HashMap<(String, String), String>,
    /// `CreateCustomModelDeployment` idempotency: `(region, clientRequestToken)` -> the
    /// `customModelDeploymentArn` that token minted. Scoped to that one operation, and to
    /// the region the request addressed, the way the Provisioned Throughput index is.
    pub custom_model_deployment_create_tokens: HashMap<(String, String), String>,
    /// How many custom model deployment ids this store has minted; the uniqueness half of
    /// a minted id.
    pub minted_custom_model_deployments: u64,
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
    /// Every stored model customization job, keyed by its `ModelCustomizationJobArn` — one
    /// of the two spellings a `ModelCustomizationJobIdentifier` carries.
    pub model_customization_jobs: HashMap<String, ModelCustomizationJobRecord>,
    /// `(region, jobName)` -> `ModelCustomizationJobArn`, so a duplicate job name is a
    /// `ConflictException` and the other `ModelCustomizationJobIdentifier` spelling — the
    /// bare job name — resolves to the job it names.
    pub model_customization_job_names: HashMap<(String, String), String>,
    /// `CreateModelCustomizationJob` idempotency: `(region, clientRequestToken)` -> the
    /// `jobArn` that token minted. Scoped to that one operation, and to the region the
    /// request addressed, the way the model import job index is.
    pub model_customization_job_create_tokens: HashMap<(String, String), String>,
    /// How many model customization job ids this store has minted; the uniqueness half of
    /// a minted id.
    pub minted_model_customization_jobs: u64,
    /// Every submitted model copy job, keyed by its `ModelCopyJobArn` — the one identifier
    /// `GetModelCopyJob` addresses a job by.
    pub model_copy_jobs: HashMap<String, ModelCopyJobRecord>,
    /// `CreateModelCopyJob` idempotency: `(region, clientRequestToken)` -> the `jobArn`
    /// that token minted. Scoped to that one operation, and to the region the request
    /// addressed, the way the model customization job index is.
    pub model_copy_job_create_tokens: HashMap<(String, String), String>,
    /// How many model copy job ids this store has minted; the uniqueness half of a minted
    /// id.
    pub minted_model_copy_jobs: u64,
    /// Every submitted batch inference job, keyed by its `ModelInvocationJobArn` — one of
    /// the two spellings a `ModelInvocationJobIdentifier` carries, the other being the
    /// minted job id, which that ARN itself carries.
    pub model_invocation_jobs: HashMap<String, ModelInvocationJobRecord>,
    /// `CreateModelInvocationJob` idempotency: `(region, clientRequestToken)` -> the
    /// `jobArn` that token minted. Scoped to that one operation, and to the region the
    /// request addressed, the way the model customization job index is.
    pub model_invocation_job_create_tokens: HashMap<(String, String), String>,
    /// How many batch inference job ids this store has minted; the uniqueness half of a
    /// minted id.
    pub minted_model_invocation_jobs: u64,
    /// Every stored Automated Reasoning policy, keyed by its
    /// `AutomatedReasoningPolicyArn` — the one identifier
    /// `GetAutomatedReasoningPolicy` addresses a policy by, which carries the region too,
    /// so the same id in another region is another key.
    pub automated_reasoning_policies: HashMap<String, AutomatedReasoningPolicyRecord>,
    /// `(region, name)` -> `AutomatedReasoningPolicyArn`, so a duplicate name is a
    /// `ConflictException`.
    ///
    /// Keyed by the region too because the ARN carries one, so the same name in another
    /// region names another policy.
    pub automated_reasoning_policy_names: HashMap<(String, String), String>,
    /// `CreateAutomatedReasoningPolicy` idempotency: `(region, clientRequestToken)` -> the
    /// `policyArn` that token minted. Scoped to that one operation, and to the region the
    /// request addressed, the way the custom model index is.
    pub automated_reasoning_policy_create_tokens: HashMap<(String, String), String>,
    /// How many Automated Reasoning policy ids this store has minted; the uniqueness half
    /// of a minted id.
    pub minted_automated_reasoning_policies: u64,
    /// Every stored Automated Reasoning policy build workflow, keyed by the
    /// `(policyArn, buildWorkflowId)` pair every one of its operations addresses it by
    /// (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyBuildWorkflow")`),
    /// so a workflow is only ever read under the policy it was started for.
    pub automated_reasoning_policy_build_workflows:
        HashMap<(String, String), AutomatedReasoningPolicyBuildWorkflowRecord>,
    /// `StartAutomatedReasoningPolicyBuildWorkflow` idempotency:
    /// `(policyArn, clientRequestToken)` -> the `buildWorkflowId` that token started.
    /// Scoped to that one operation, and to the policy the request addressed, the way the
    /// policy create index is scoped to the region.
    pub automated_reasoning_policy_build_workflow_tokens: HashMap<(String, String), String>,
    /// How many build workflow ids this store has minted; the uniqueness half of a minted
    /// id.
    pub minted_automated_reasoning_policy_build_workflows: u64,
    /// Every stored Automated Reasoning policy test, keyed by
    /// `(AutomatedReasoningPolicyArn, testCaseId)` — the pair
    /// `GetAutomatedReasoningPolicyTestCase` addresses a test by, because a test belongs to
    /// the policy it was created under and is only ever read through it.
    pub automated_reasoning_policy_test_cases:
        HashMap<(String, String), AutomatedReasoningPolicyTestCaseRecord>,
    /// `CreateAutomatedReasoningPolicyTestCase` idempotency:
    /// `(AutomatedReasoningPolicyArn, clientRequestToken)` -> the `testCaseId` that token
    /// minted. Scoped to that one operation, and to the policy the request addressed, the
    /// way the test store itself is.
    pub automated_reasoning_policy_test_case_create_tokens: HashMap<(String, String), String>,
    /// How many Automated Reasoning policy test ids this store has minted; the uniqueness
    /// half of a minted id.
    pub minted_automated_reasoning_policy_test_cases: u64,
    /// Every run of a stored test, keyed by the
    /// `(AutomatedReasoningPolicyArn, buildWorkflowId, testCaseId)` triple
    /// `GetAutomatedReasoningPolicyTestResult` addresses a result by, because a run belongs
    /// to the build workflow it tested and to the test it ran.
    pub automated_reasoning_policy_test_runs:
        HashMap<(String, String, String), AutomatedReasoningPolicyTestRunRecord>,
    /// `StartAutomatedReasoningPolicyTestWorkflow` idempotency:
    /// `(AutomatedReasoningPolicyArn, buildWorkflowId)` -> the `clientRequestToken` of the
    /// test workflow that build workflow last accepted, with the `testCaseIds` that request
    /// carried (`None` when it named none, which selects every test of the policy), so the
    /// request that token and that selection already started schedules nothing new.
    ///
    /// The SELECTION is part of the entry because a request naming other tests asks for
    /// other work, which a stored token must not answer as already done.
    ///
    /// ONE entry per build workflow rather than one per token, which is what keeps this
    /// index bounded by the build workflows the quota already admits: an accepted test
    /// workflow re-runs the runs it finds rather than minting a record a token could be
    /// counted against, so a per-token index would grow with nothing but fresh tokens. A
    /// token the next accepted test workflow of that build workflow supersedes is therefore
    /// no longer a replay.
    pub automated_reasoning_policy_test_workflow_tokens:
        HashMap<(String, String), AcceptedAutomatedReasoningPolicyTestWorkflow>,
    /// Every stored application inference profile, keyed by its `InferenceProfileArn` —
    /// one of the two spellings an `InferenceProfileIdentifier` carries.
    pub inference_profiles: HashMap<String, InferenceProfileRecord>,
    /// `CreateInferenceProfile` idempotency: `(region, clientRequestToken)` -> the
    /// `inferenceProfileArn` that token minted. Scoped to that one operation, and to the
    /// region the request addressed, the way the prompt router index is.
    pub inference_profile_create_tokens: HashMap<(String, String), String>,
    /// How many inference profile ids this store has minted; the uniqueness half of a
    /// minted id.
    pub minted_inference_profiles: u64,
    /// The foundation models this service offers, seeded from [`FOUNDATION_MODELS`] by
    /// [`FoundationModelCatalog::default`].
    ///
    /// A FIXED set, not a resource a caller creates: no operation of this service adds a
    /// row to it or takes one away, so it cannot grow the store either.
    pub foundation_models: FoundationModelCatalog,
    /// Whether this account reaches every model the catalog offers by default
    /// ([`FoundationModelAccessDefault`]), which is what a listing of the catalog reports
    /// beside the models themselves.
    ///
    /// Seeded, like the catalog beside it, and written by no operation of this service.
    pub foundation_model_access_default: FoundationModelAccessDefault,
    /// Every resource policy in force, keyed by the RESOURCE it is attached to rather than
    /// by one spelling of that resource's ARN — the document `PutResourcePolicy` last
    /// stored for it and `GetResourcePolicy` reports back.
    ///
    /// Read through [`Self::resource_policy`], attached by
    /// [`Self::attach_resource_policy`] — which attaches a policy only to a resource this
    /// store HOLDS ([`Self::policy_resource`]) — and taken away by
    /// [`Self::detach_resource_policy`]; both drop the entries whose resource is
    /// gone, so this map holds at most one entry per live resource of this account and a
    /// create/delete cycle cannot grow it.
    pub resource_policies: HashMap<TaggedResource, String>,
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
    /// The account's data retention setting, as `PutAccountDataRetention` last stored it,
    /// or `None` while no put has set one — in which case the mode in force is
    /// [`DATA_RETENTION_MODE_DEFAULT`].
    ///
    /// ONE record for the reason [`AccountDataRetention`] gives.
    pub account_data_retention: Option<AccountDataRetention>,
}

/// A resource of this service that carries tags, as the record holding those tags is
/// keyed.
///
/// What [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::tagged_resource`]
/// resolves a tagging call's `resourceARN` to, so the ARN is read ONCE and every tagging
/// operation then reaches the same record. Each variant carries the key its family's store
/// is keyed by, which is why an imported model carries the ARN of the JOB that imports it:
/// that job is where the model's tags live, as [`ModelImportJobRecord`] states.
///
/// `Hash` and `Eq` are what let it key
/// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::resource_policies`]: a
/// resource policy is attached to the RESOURCE this resolves to, so two spellings of one
/// resource's ARN address the same policy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    /// A custom model deployment, by its `CustomModelDeploymentArn`.
    CustomModelDeployment(String),
    /// An application inference profile, by its `InferenceProfileArn`.
    InferenceProfile(String),
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

impl ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState {
    /// Mint a fresh custom model deployment id: the resource half of a
    /// `custom-model-deployment/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `CustomModelDeploymentArn` admits exactly `[a-z0-9]{12}` there, which is what two
    /// [`ID_HALF_WIDTH`] base-36 halves spell.
    pub fn mint_custom_model_deployment_id(&mut self, now: SystemTime) -> String {
        self.minted_custom_model_deployments += 1;
        minted_id(self.minted_custom_model_deployments, now)
    }

    /// The `customModelDeploymentArn` a `CustomModelDeploymentIdentifier` names, within the
    /// region the request addresses: the identifier itself when it is the ARN of a
    /// deployment stored for `region`, otherwise the ARN the deployment of that NAME in
    /// `region` carries.
    ///
    /// The identifier's pattern is that ARN OR a bare `ModelDeploymentName`
    /// (`smithy_query(query="operation",operation="GetCustomModelDeployment")`), so both
    /// spellings address the same deployment — and BOTH are scoped to the region, because a
    /// deployment belongs to the account-region it was created in: an AWS endpoint serves
    /// one region, so a deployment of another one is not a resource this request can
    /// address, whichever spelling names it.
    #[must_use]
    pub fn custom_model_deployment_arn_of(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<String> {
        if let Some(record) = self.custom_model_deployments.get(identifier) {
            return (record.region == region).then(|| record.arn.clone());
        }
        self.custom_model_deployment_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The custom model deployment a `CustomModelDeploymentIdentifier` names, with any
    /// status transition the clock has made due at `now` materialised first — so every
    /// caller reads the status the deployment holds at `now`, whichever operation asked.
    pub fn custom_model_deployment(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut CustomModelDeploymentRecord> {
        let arn = self.custom_model_deployment_arn_of(identifier, region)?;
        let record = self.custom_model_deployments.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every custom model deployment status transition the clock has made due
    /// at `now`, so a LISTING reports — and filters on — the status each record holds at
    /// `now` exactly as a read of that one deployment would.
    pub fn settle_custom_model_deployments(&mut self, now: SystemTime) {
        for record in self.custom_model_deployments.values_mut() {
            record.settle(now);
        }
    }
}

/// The ARN of a custom model deployment in `region`:
/// `arn:aws:bedrock:<region>:<account>:custom-model-deployment/<id>`, as
/// `CustomModelDeploymentArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model-deployment/[a-z0-9]{12}$`)
/// spells it.
#[must_use]
pub fn custom_model_deployment_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("{CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE}/{id}"),
    )
}

/// The ARN resource type a custom model deployment is named by.
pub const CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE: &str = "custom-model-deployment";

/// What this service answers as its `ValidationException` message for a
/// `CustomModelDeploymentIdentifier` that is not the ARN of a deployment and resolves to
/// none.
///
/// RECORDED behaviour, not a modelled rule: the corpus `simparity-behavioral-ga-prod-v1`
/// records Amazon Bedrock answering 400 `ValidationException` with exactly this sentence
/// for a bare deployment name this account holds no deployment under. A CONSTANT,
/// interpolating no part of the request: it reports that the identifier is not usable, and
/// nothing about which name was asked for, so the message cannot be used to probe the
/// store.
pub const CUSTOM_MODEL_DEPLOYMENT_IDENTIFIER_INVALID: &str =
    "The provided custom model deployment identifier is invalid.";

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

/// A minted UUIDv4-shaped resource id: the lowercase hyphenated form
/// `AutomatedReasoningPolicyBuildWorkflowId` spells.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2` crate)
/// and hex-rendered (the `hex` crate), the way this repo's other services mint a
/// UUID-shaped id (see
/// `services/service-com-amazonaws-wafv2-awswaf-20190729/src/state.rs`): this store's mint
/// `ordinal`, so two ids of one store can never collide; `now` in nanoseconds; and the
/// OS-seeded randomness a `RandomState` draws, so an id is not a guessable sequence
/// number. RFC 4122 §4.4's version nibble and variant bits are then set, which is what the
/// `4` and the `[89ab]` in that pattern require. An id names a resource; a credential
/// would come from `ctx.credential_store()`, never here.
fn minted_uuid(ordinal: u64, now: SystemTime) -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::collections::hash_map::RandomState;
    use ::std::hash::BuildHasher;

    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(ordinal).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(ordinal.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..]
    )
}

/// What `UpdateMarketplaceModelEndpoint`, `RegisterMarketplaceModelEndpoint` and
/// `DeregisterMarketplaceModelEndpoint` answer as the `ResourceNotFoundException` message
/// for a Bedrock Marketplace model endpoint ARN this account holds no endpoint for.
///
/// RECORDED behaviour, not a modelled sentence (the model's own doc for the error reads
/// "The specified resource Amazon Resource Name (ARN) was not found. …"): the corpus
/// `simparity-behavioral-ga-prod-v1` records Amazon Bedrock answering 404
/// `ResourceNotFoundException` with exactly this sentence for an `endpointArn` that names
/// no endpoint. A CONSTANT, interpolating no part of the request — it reports that the
/// resource is not there and nothing about which ARN was asked for, so the message cannot
/// be used to probe the store.
///
/// Scoped to the operations recorded answering `ResourceNotFoundException` for such an ARN
/// — the WRITES that address an endpoint by it. The same corpus records
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

/// Every Bedrock Marketplace model this emulator's Amazon Bedrock publishes, by the HUB
/// CONTENT NAME the Amazon SageMaker public hub holds it under — the name a
/// `ModelSourceIdentifier` addresses a model by.
///
/// A PUBLISHED catalog, the way [`FOUNDATION_MODELS`] is: Amazon Bedrock Marketplace offers
/// the models of the AWS-managed `SageMakerPublicHub`, and no request creates one, so this
/// is the set a `modelSourceIdentifier` can name — nothing a caller adds to or removes
/// from. The names are the hub content names AWS publishes there, and the catalog is a
/// SUBSET of them, spanning several providers and name forms, because the rule — a name the
/// public hub does not publish names no model — not the catalog's size is what this
/// emulator has to get right.
///
/// In `name` order, so a reader sees the set rather than a history of additions.
pub const PUBLISHED_MARKETPLACE_MODELS: &[&str] = &[
    "deepseek-llm-r1-distill-qwen-7b",
    "huggingface-llm-falcon-7b-instruct-bf16",
    "huggingface-llm-gemma-7b-instruct",
    "huggingface-llm-mistral-7b-instruct",
    "meta-textgeneration-llama-2-7b-f",
    "meta-textgeneration-llama-3-70b-instruct",
    "meta-textgeneration-llama-3-8b-instruct",
];

/// The hub content NAME inside a `ModelSourceIdentifier`: the segment after the ARN's
/// `.../Model/`, up to the content VERSION the identifier may carry after it — or the whole
/// value when it carries no such segment.
///
/// `ModelSourceIdentifier`'s pattern is
/// `arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*`
/// (`smithy_query(query="shape",shape="ModelSourceIdentifier")`), and a SageMaker hub
/// content is addressed by `<hub>/<type>/<name>/<version>`, so what follows `Model/` is the
/// content's name and then its version. Built like [`marketplace_endpoint_name_of`]: a
/// member carrying a resource's ARN is read back to the name the resource is held under.
pub fn marketplace_model_source_name_of(model_source_identifier: &str) -> &str {
    let after_type = match model_source_identifier.rsplit_once("/Model/") {
        Some((_, rest)) => rest,
        None => model_source_identifier,
    };
    match after_type.split_once('/') {
        Some((name, _version)) => name,
        None => after_type,
    }
}

/// Whether the Amazon SageMaker public hub publishes a Bedrock Marketplace model under this
/// hub content name ([`PUBLISHED_MARKETPLACE_MODELS`]).
#[must_use]
pub fn is_published_marketplace_model(hub_content_name: &str) -> bool {
    PUBLISHED_MARKETPLACE_MODELS.contains(&hub_content_name)
}

/// What a request naming a Bedrock Marketplace model the public hub does not publish is
/// refused with: `Hub content with name <name> does not exist.`
///
/// RECORDED behaviour, not a modelled sentence: the corpus records Amazon Bedrock answering
/// `RegisterMarketplaceModelEndpoint` 404 `ResourceNotFoundException` with exactly this
/// sentence for a well-formed `modelSourceIdentifier` whose hub content name the public hub
/// holds nothing under — and it is the model source, not the endpoint, that such a request
/// is refused on. BUILT FROM THE REQUEST: it names the hub content the identifier named
/// ([`marketplace_model_source_name_of`]), so the rule is "name the content the identifier
/// names", not one recorded name.
pub fn marketplace_hub_content_not_found(hub_content_name: &str) -> String {
    format!("Hub content with name {hub_content_name} does not exist.")
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

/// One stored prompt router as the `PromptRouterSummary` a listing carries.
///
/// Lives here rather than in `ops/list_prompt_routers.rs` because every member it carries
/// is also a member `GetPromptRouter` reports, and the two must agree about them: a listed
/// summary is the same router the read of that one router finds. It is also the shape the
/// published routers are built in ([`default_prompt_routers`]), so a caller-created router
/// and a published one are reported through ONE mapping.
#[must_use]
pub fn prompt_router_summary(router: &PromptRouter) -> PromptRouterSummary {
    PromptRouterSummary {
        prompt_router_name: router.name.clone(),
        routing_criteria: router.routing_criteria.clone(),
        description: router.description.clone(),
        created_at: Some(date_time(router.created_at)),
        updated_at: Some(date_time(router.updated_at)),
        prompt_router_arn: router.arn.clone(),
        models: router.models.clone(),
        fallback_model: router.fallback_model.clone(),
        status: router.status.to_string(),
        r#type: router.router_type.to_string(),
    }
}

/// The ARN of an application inference profile in `region`:
/// `arn:aws:bedrock:<region>:<account>:application-inference-profile/<id>`.
///
/// `application-inference-profile` is the resource type `InferenceProfileArn` admits for a
/// profile a caller created — its pattern is
/// `^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):(inference-profile|application-inference-profile)/[a-zA-Z0-9-:.]+$`
/// (`smithy_query(query="shape",shape="InferenceProfileSummary")`) — while
/// `inference-profile` is the type the system-defined profiles Amazon Bedrock itself
/// publishes carry, and no operation here mints one of those.
pub fn application_inference_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("application-inference-profile/{id}"),
    )
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
/// The `AutomatedReasoningPolicyArn` of the policy `id` names in `region`:
/// `arn:aws:bedrock:<region>:<account>:automated-reasoning-policy/<id>`, which is the
/// form `AutomatedReasoningPolicyArn` spells without its optional version suffix.
#[must_use]
pub fn automated_reasoning_policy_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("automated-reasoning-policy/{id}"),
    )
}

/// The versioned `AutomatedReasoningPolicyArn` of version `version` of the policy
/// `policy_arn` names: the base ARN with the `:<version>` suffix
/// `AutomatedReasoningPolicyArn` admits after it.
#[must_use]
pub fn automated_reasoning_policy_version_arn(policy_arn: &str, version: u32) -> String {
    format!("{policy_arn}:{version}")
}

/// An `AutomatedReasoningPolicyArn` split into the base ARN and the version its optional
/// `:<version>` suffix names.
///
/// The suffix is the LAST colon-separated field, and only when it spells a number the
/// shape admits there (`([1-9][0-9]{0,11})`); anything else is part of the base ARN, so an
/// ARN carrying no suffix is returned whole.
#[must_use]
pub fn automated_reasoning_policy_arn_parts(policy_arn: &str) -> (&str, Option<u32>) {
    match policy_arn.rsplit_once(':') {
        Some((base, suffix)) => match version_number(suffix) {
            Some(number) => (base, Some(number)),
            None => (policy_arn, None),
        },
        None => (policy_arn, None),
    }
}

/// The `AutomatedReasoningPolicyTestCase` a stored test is reported as.
///
/// ONE mapping, so the read of a single test and a listing of a policy's tests report the
/// same test the same way.
#[must_use]
pub fn automated_reasoning_policy_test_case(
    record: &AutomatedReasoningPolicyTestCaseRecord,
) -> AutomatedReasoningPolicyTestCase {
    AutomatedReasoningPolicyTestCase {
        test_case_id: record.id.clone(),
        guard_content: record.guard_content.clone(),
        query_content: record.query_content.clone(),
        expected_aggregated_findings_result: Some(
            record.expected_aggregated_findings_result.clone(),
        ),
        confidence_threshold: record.confidence_threshold,
        created_at: date_time(record.created_at),
        updated_at: date_time(record.updated_at),
    }
}

/// The `AutomatedReasoningPolicyTestResult` a stored run of a stored test is reported as.
///
/// ONE mapping, so the read of a single result and a listing of a build workflow's results
/// report the same run the same way. The test it ran is reported through
/// [`automated_reasoning_policy_test_case`], so a result carries exactly the test a read of
/// that test reports.
///
/// `testFindings`, `aggregatedTestFindingsResult` and `testRunResult` are reported only
/// once something has checked the test's content against the policy's logic. Nothing in
/// this emulator does, so a run reports its lifecycle — the status it holds and when it
/// last changed — and none of the three: every one of them is an OPTIONAL member
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyTestResult")`), so a result
/// that carries no finding is one the shape admits, and inventing a finding or a verdict
/// here would report an outcome no check produced.
#[must_use]
pub fn automated_reasoning_policy_test_result(
    run: &AutomatedReasoningPolicyTestRunRecord,
    test_case: &AutomatedReasoningPolicyTestCaseRecord,
) -> AutomatedReasoningPolicyTestResult {
    AutomatedReasoningPolicyTestResult {
        test_case: automated_reasoning_policy_test_case(test_case),
        policy_arn: run.policy_arn.clone(),
        test_run_status: run.status.to_string(),
        test_findings: None,
        test_run_result: None,
        aggregated_test_findings_result: None,
        updated_at: date_time(run.updated_at),
    }
}

/// The `definitionHash` of an Automated Reasoning policy definition: 128 digits of
/// `[0-9a-z]`, as `AutomatedReasoningPolicyHash` requires.
///
/// A function of the DEFINITION alone, so two policies whose definitions agree report one
/// hash and any change to a definition changes it — which is what the member documents it
/// for ("used as a concurrency token for creating policy versions"). The digest is SHA-512
/// hex, whose 128 lowercase hex digits are exactly the width and alphabet the shape
/// admits, taken over a canonical rendering of the definition rather than over the request
/// bytes, so the same definition written two ways hashes alike.
///
/// An ABSENT definition hashes as the empty definition the shape's own defaults spell: a
/// policy created without one still reports a hash, and reports the same one until a
/// definition is written to it.
#[must_use]
pub fn policy_definition_hash(definition: Option<&AutomatedReasoningPolicyDefinition>) -> String {
    use ::sha2::{Digest, Sha512};

    let canonical = canonical_policy_definition(definition).to_string();
    ::hex::encode(Sha512::digest(canonical.as_bytes()))
}

/// A policy definition as its CONTENT: the format version it carries or the one the shape
/// declares as its `@default` ([`POLICY_DEFINITION_FORMAT_VERSION`]), and each of its three
/// lists, an absent one read as the empty list.
///
/// So a revision that holds NO definition, and one whose definition carries no types,
/// variables or rules, are one definition — which is what makes [`policy_definition_hash`]
/// a function of the definition's content alone, and what
/// `ops/export_automated_reasoning_policy_version.rs` exports, so the definition that
/// operation reports always hashes to the `definitionHash` the same revision reports.
#[must_use]
pub fn policy_definition_content(
    definition: Option<&AutomatedReasoningPolicyDefinition>,
) -> AutomatedReasoningPolicyDefinition {
    AutomatedReasoningPolicyDefinition {
        version: definition
            .map(|definition| definition.version.clone())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| POLICY_DEFINITION_FORMAT_VERSION.to_string()),
        types: Some(
            definition
                .and_then(|definition| definition.types.clone())
                .unwrap_or_default(),
        ),
        variables: Some(
            definition
                .and_then(|definition| definition.variables.clone())
                .unwrap_or_default(),
        ),
        rules: Some(
            definition
                .and_then(|definition| definition.rules.clone())
                .unwrap_or_default(),
        ),
    }
}

/// A policy definition as the one JSON value [`policy_definition_hash`] digests: every
/// member of every element of [`policy_definition_content`] in a fixed order, so the
/// rendering depends on the definition's CONTENT and on nothing else.
fn canonical_policy_definition(
    definition: Option<&AutomatedReasoningPolicyDefinition>,
) -> ::serde_json::Value {
    let content = policy_definition_content(definition);
    let types: Vec<::serde_json::Value> = content
        .types
        .iter()
        .flatten()
        .map(|declared| {
            ::serde_json::json!({
                "name": declared.name,
                "description": declared.description,
                "values": declared
                    .values
                    .iter()
                    .map(|value| ::serde_json::json!({
                        "value": value.value,
                        "description": value.description,
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let variables: Vec<::serde_json::Value> = content
        .variables
        .iter()
        .flatten()
        .map(|variable| {
            ::serde_json::json!({
                "name": variable.name,
                "type": variable.r#type,
                "description": variable.description,
            })
        })
        .collect();
    let rules: Vec<::serde_json::Value> = content
        .rules
        .iter()
        .flatten()
        .map(|rule| {
            ::serde_json::json!({
                "id": rule.id,
                "expression": rule.expression,
                "alternateExpression": rule.alternate_expression,
            })
        })
        .collect();
    ::serde_json::json!({
        "version": content.version,
        "types": types,
        "variables": variables,
        "rules": rules,
    })
}

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

/// The ARN of an advanced prompt optimization job in `region`:
/// `arn:aws:bedrock:<region>:<account>:advanced-prompt-optimization-job/<id>`, as
/// `AdvancedPromptOptimizationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/[a-z0-9]{12}$`)
/// spells it.
pub fn advanced_prompt_optimization_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("advanced-prompt-optimization-job/{id}"),
    )
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

/// The base models offered with Provisioned Throughput and NO commitment term.
///
/// Whether a purchase may omit `commitmentDuration` is a property of the MODEL, not of the
/// account: that member's own documentation sends a caller to "Supported Regions and models
/// for Provisioned Throughput" for "which base models support no commitment"
/// (`smithy_query(query="doc",shape="CreateProvisionedModelThroughputRequest",member="commitmentDuration")`,
/// which also states that custom models support every level of commitment). The base models
/// that page lists a no-commitment purchase for are the earlier generations — the Amazon
/// Titan family, the Cohere models, Mixtral and SDXL — so a later model is offered with
/// Provisioned Throughput only under a term, and a no-commitment purchase over one of those
/// is refused rather than drawn against the no-commitment model-unit quota
/// ([`granted_model_units`]).
///
/// Kept as a list of ids rather than a column of [`FOUNDATION_MODELS`] because it is a
/// different published table from the per-model attributes those rows carry. Every id here
/// is a row of that catalog whose `inferenceTypesSupported` carries
/// [`INFERENCE_TYPE_PROVISIONED`], which is what
/// [`supports_no_commitment_provisioned_throughput`] holds it to.
pub const NO_COMMITMENT_PROVISIONED_MODELS: &[&str] = &[
    "amazon.titan-embed-image-v1",
    "amazon.titan-embed-text-v2:0",
    "amazon.titan-image-generator-v1",
    "amazon.titan-text-lite-v1",
    "cohere.command-light-text-v14",
    "cohere.command-text-v14",
    "cohere.embed-english-v3",
    "mistral.mixtral-8x7b-instruct-v0:1",
    "stability.stable-diffusion-xl-v1",
];

/// Whether a Provisioned Throughput over `model` can be bought with NO commitment term
/// ([`NO_COMMITMENT_PROVISIONED_MODELS`]).
///
/// A model that is offered with no Provisioned Throughput at all is offered with none
/// without a commitment either, so this answers for the catalog row as a whole rather than
/// for the id alone.
#[must_use]
pub fn supports_no_commitment_provisioned_throughput(model: &FoundationModel) -> bool {
    supports_provisioned_throughput(model)
        && NO_COMMITMENT_PROVISIONED_MODELS.contains(&model.model_id)
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

/// The `CustomModelDeploymentSummary` a listing reports for a stored custom model
/// deployment.
///
/// Lives here rather than in `ops/list_custom_model_deployments.rs` because every member it
/// carries is also a member `GetCustomModelDeployment` reports, and the two must agree
/// about them: a listed summary is the same record the read of that one deployment finds.
#[must_use]
pub fn custom_model_deployment_summary(
    record: &CustomModelDeploymentRecord,
) -> CustomModelDeploymentSummary {
    CustomModelDeploymentSummary {
        custom_model_deployment_arn: record.arn.clone(),
        custom_model_deployment_name: record.name.clone(),
        model_arn: record.model_arn.clone(),
        created_at: date_time(record.created_at),
        status: record.status.to_string(),
        last_updated_at: record.last_updated_at.map(date_time),
        // `failureMessage` is reported "if the deployment status is FAILED". Nothing here
        // makes an accepted deployment fail, so there is none to report.
        failure_message: None,
    }
}

/// The ARN resource type a custom model carries: the field `CustomModelArn` spells before
/// the base-model segment and the model's own id
/// (`smithy_query(query="operation",operation="GetCustomModel")`).
///
/// Stated once because two layers read it — the ARN this service builds for one, and the
/// test that tells an ARN from the bare NAME a `ModelIdentifier` also admits
/// ([`names_arn_of`]).
pub const CUSTOM_MODEL_RESOURCE_TYPE: &str = "custom-model";

/// Whether `value` is an ARN of this service's own namespace naming a resource of
/// `resource_type`, rather than one of the other spellings a member admits.
///
/// The question a member whose `@pattern` admits an ARN **or** a bare name has to answer
/// before anything else: which of the two a caller sent. The parse is
/// `service_common::arn`'s one parse, so no two readings of "is this an ARN of ours"
/// disagree.
#[must_use]
pub fn names_arn_of(value: &str, resource_type: &str) -> bool {
    ::service_common::arn::parse_of(value, ARN_NAMESPACE)
        .is_some_and(|arn| arn.resource_type() == resource_type)
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

/// The base-model segment a `CustomModelArn` embeds: the text between that ARN's
/// `custom-model/` resource type and the custom model's own id, which is either
/// [`CUSTOM_MODEL_IMPORTED_BASE`] or the id of the foundation model the custom model was
/// customized from, with the version qualifiers AWS appends to it.
///
/// `None` for a value that is not a `custom-model` ARN of this service's namespace at all;
/// such a value is refused by the member's own modelled constraints before any handler
/// sees it.
///
/// A TOTAL read of the value: the shared six-field ARN parse
/// (`service_common::arn::parse_of`, which answers `None` for anything that is not an ARN
/// of this service's namespace), then `strip_prefix` and `rsplit_once`, each answering
/// `None` rather than panicking. Nothing is indexed, sliced or unwrapped, so no value a
/// caller can send reaches a panic here.
///
/// Lives here because every operation that takes a custom model's ARN as a MODEL SOURCE
/// reads the same segment out of it and refuses the same way
/// ([`invalid_base_model_message`]): `ops/create_custom_model_deployment.rs` and
/// `ops/create_model_copy_job.rs` today.
#[must_use]
pub fn custom_model_base_segment(model_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(model_arn, ARN_NAMESPACE)?;
    let rest = arn.resource.strip_prefix("custom-model/")?;
    let (segment, id) = rest.rsplit_once('/')?;
    (!segment.is_empty() && !id.is_empty()).then_some(segment)
}

/// What this service answers as its `ValidationException` message for a `CustomModelArn`
/// whose base-model segment names no foundation model the published catalog carries.
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus records Amazon Bedrock naming the base model the submitted ARN embedded
/// ([`custom_model_base_segment`]), quoted, in exactly this sentence. So the rule is "name
/// the base model the ARN embeds", not one recorded model id.
#[must_use]
pub fn invalid_base_model_message(base_model_segment: &str) -> String {
    format!(
        "The foundation model '{base_model_segment}' for custom model is not a valid \
         foundation model"
    )
}

/// The ARN of a model copy job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-copy-job/<id>`, as `ModelCopyJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-copy-job/[a-z0-9]{12}$`)
/// spells it.
#[must_use]
pub fn model_copy_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("model-copy-job/{id}"))
}

/// One stored model copy job as the `ModelCopyJobSummary` a listing carries.
///
/// Lives here rather than in `ops/list_model_copy_jobs.rs` because every member it carries
/// is also a member `GetModelCopyJob` reports, and the two must agree about them: a listed
/// summary is the same job the read of that one job finds.
///
/// `failureMessage` is reported for a copy that FAILED; nothing here makes an accepted copy
/// fail, so there is none to report. `targetModelTags` admits zero entries, so a copy
/// carrying no tags reports the empty list rather than omitting the member.
#[must_use]
pub fn model_copy_job_summary(job: &ModelCopyJobRecord) -> ModelCopyJobSummary {
    ModelCopyJobSummary {
        job_arn: job.arn.clone(),
        status: job.status.to_string(),
        creation_time: date_time(job.created_at),
        target_model_arn: job.target_model_arn.clone(),
        target_model_name: Some(job.target_model_name.clone()),
        source_account_id: job.source_account_id.clone(),
        source_model_arn: job.source_model_arn.clone(),
        target_model_kms_key_arn: job.target_model_kms_key_arn.clone(),
        target_model_tags: Some(job.target_model_tags.clone()),
        failure_message: None,
        source_model_name: job.source_model_name.clone(),
    }
}

/// The ARN of a model customization job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-customization-job/<base model>/<id>`, as
/// `ModelCustomizationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-customization-job/[a-z0-9-]{1,63}[.]{1}…/[a-z0-9]{12}$`)
/// spells it.
///
/// `base_model_id` is the base-model segment that pattern carries before the id — the
/// `modelId` of the catalog row the request's `baseModelIdentifier` resolved to, exactly as
/// the custom model the job produces carries it in [`custom_model_arn`].
pub fn model_customization_job_arn(region: &str, base_model_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("model-customization-job/{base_model_id}/{id}"),
    )
}

/// The `CustomizationType` a create request's `customizationType` names, or
/// [`CUSTOMIZATION_TYPE_FINE_TUNING`] when it names none; `None` for a value the modelled
/// enum does not carry.
///
/// Resolved against `crate::ops::types::CUSTOMIZATION_TYPE_VALUES` — the enum's own value
/// list — so a stored record carries one of the modelled values rather than whatever text
/// arrived.
#[must_use]
pub fn customization_type_of(requested: Option<&str>) -> Option<&'static str> {
    let Some(requested) = requested else {
        return Some(CUSTOMIZATION_TYPE_FINE_TUNING);
    };
    crate::ops::types::CUSTOMIZATION_TYPE_VALUES
        .iter()
        .find(|value| **value == requested)
        .copied()
}

/// Whether `model` is published as supporting the customization `customization_type` names,
/// which is what decides whether a customization job can be created over it.
///
/// [`FoundationModel::customizations_supported`] is the `ModelCustomizationList`
/// `ListFoundationModels` reports for that model, each value out of `ModelCustomization`
/// (`crate::ops::types::MODEL_CUSTOMIZATION_VALUES`) — so this reads the published catalog
/// rather than any property of the request, exactly as
/// [`supports_provisioned_throughput`] does for a Provisioned Throughput.
#[must_use]
pub fn supports_customization(model: &FoundationModel, customization_type: &str) -> bool {
    model
        .customizations_supported
        .contains(&customization_type)
}

/// The ARN of a batch inference job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-invocation-job/<id>`, as
/// `ModelInvocationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-invocation-job/[a-z0-9]{12}$`)
/// spells it.
pub fn model_invocation_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("model-invocation-job/{id}"))
}

/// The `ModelInvocationType` a create request's `modelInvocationType` names; `None` for a
/// value the modelled enum does not carry.
///
/// Resolved against `crate::ops::types::MODEL_INVOCATION_TYPE_VALUES` — the enum's own
/// value list — so a stored record carries one of the modelled values rather than whatever
/// text arrived, exactly as [`customization_type_of`] does for a customization.
#[must_use]
pub fn model_invocation_type_of(requested: &str) -> Option<&'static str> {
    crate::ops::types::MODEL_INVOCATION_TYPE_VALUES
        .iter()
        .find(|value| **value == requested)
        .copied()
}

/// The `ModelInvocationJobSummary` a listing reports for a stored batch inference job.
///
/// Lives here rather than in `ops/list_model_invocation_jobs.rs` because every member it
/// carries is also a member `GetModelInvocationJob` reports, and the two must agree about
/// them: a listed summary is the same record the read of that one job finds.
#[must_use]
pub fn model_invocation_job_summary(record: &ModelInvocationJobRecord) -> ModelInvocationJobSummary {
    ModelInvocationJobSummary {
        job_arn: record.arn.clone(),
        job_name: record.name.clone(),
        model_id: record.model_id.clone(),
        client_request_token: record.client_request_token.clone(),
        role_arn: record.role_arn.clone(),
        status: Some(record.status.to_string()),
        // "If the batch inference job failed, this field contains a message describing why
        // the job failed": the sentence its validation or its run failed on
        // ([`ModelInvocationJobRecord::message`]), and nothing for a job that has not
        // failed.
        message: record.message.clone(),
        submit_time: date_time(record.submit_time),
        last_modified_time: Some(date_time(record.last_modified_at)),
        end_time: record.ended_at.map(date_time),
        input_data_config: record.input_data_config.clone(),
        output_data_config: record.output_data_config.clone(),
        vpc_config: record.vpc_config.clone(),
        timeout_duration_in_hours: record.timeout_duration_in_hours,
        job_expiration_time: record.job_expiration_time.map(date_time),
        model_invocation_type: Some(record.model_invocation_type.to_string()),
        // The record counts are counted BY the job's own work step: the records read out of
        // the input object the submission was pointed at ([`model_invocation_run`]) and the
        // tally of the run that invoked the model over them. A job nothing is known about
        // the input of has no count to report, which is the one case all four are absent in
        // — never a job that ran.
        total_record_count: record.total_record_count,
        processed_record_count: record
            .total_record_count
            .map(|_| record.processed_record_count),
        success_record_count: record
            .total_record_count
            .map(|_| record.success_record_count),
        error_record_count: record.total_record_count.map(|_| record.error_record_count),
    }
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

/// What this service answers for a value submitted to a member whose modelled form admits
/// NOTHING BUT an ARN, when that value is not a well-formed ARN at all.
///
/// RECORDED behaviour: Amazon Bedrock reads such a member as an ARN itself, ahead of the
/// `@pattern` the model declares for it, and reports that the ARN cannot be read. A
/// CONSTANT, interpolating no part of the request: it carries neither the rejected value,
/// nor the member that carried it, nor the expression that member declares.
pub const ARN_INVALID: &str = "Provided ARN is invalid.";

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
    CUSTOM_MODEL_RESOURCE_TYPE,
    "model-copy-job",
    "model-invocation-job",
    "provisioned-model",
];

/// Whether a declared `@pattern` spells the ARN of a resource this service validates
/// itself ([`SELF_VALIDATED_RESOURCE_TYPES`]).
///
/// Read off the expression rather than off a value, because it answers a question about
/// the MEMBER: which resource the member is declared to carry an ARN of. That is what
/// decides whether a value submitted to it is read as an ARN at all — an
/// `evaluation-job` identifier is spelled as an ARN too, and the corpus records the
/// framework's constraint envelope for a value outside its form, which is why this is the
/// same catalog and not "every ARN-valued member".
///
/// Matched on `:<type>/` — the resource field as every ARN pattern of this service spells
/// it, behind the account field's colon — so one resource type is never read as the prefix
/// of another (`:custom-model/` is not `:custom-model-deployment/`).
#[must_use]
pub fn pattern_names_self_validated_resource(pattern: &str) -> bool {
    SELF_VALIDATED_RESOURCE_TYPES
        .iter()
        .any(|resource_type| pattern.contains(&format!(":{resource_type}/")))
}

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
    let words = spelled_resource_type(resource_type);
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

/// Whether the `lastUpdatedAt` concurrency token a request carried names the write the
/// addressed resource holds at `held`.
///
/// Stated once here because every resource of this service that is written under such a
/// token is held against it the same way, so two renderings of the comparison could drift
/// and one operation would then accept a token another refuses.
///
/// The comparison is at MILLISECOND resolution, and not finer: the token is a value the
/// caller echoes back from a read of the same resource, and a `date-time` instant travels
/// through clients that carry milliseconds, so holding the echoed value to the nanosecond
/// the store minted would refuse a caller naming the very write it just read. A millisecond
/// still distinguishes two writes, which is what the token is for.
#[must_use]
pub fn names_write_at(token: &::aws_smithy_types::DateTime, held: SystemTime) -> bool {
    epoch_millis(token) == epoch_millis(&date_time(held))
}

/// The instant a wire timestamp names, in whole milliseconds since the epoch.
fn epoch_millis(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000 + i128::from(at.subsec_nanos() / 1_000_000)
}

// =============================================================================
// The cursor every creation-time listing of this service pages with
// =============================================================================
//
// `ops/list_provisioned_model_throughputs.rs` and `ops/list_custom_models.rs` sort by the
// one instant their `sortBy` admits and page with a `nextToken`, and
// `ops/list_inference_profiles.rs` pages the same way over the instant its profiles were
// created, so they need the SAME cursor: stated once here, because two renderings of it
// could drift and a token issued by one shape would then not resume the listing it came
// from.

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

/// One stored application inference profile as the `InferenceProfileSummary` a listing
/// carries.
///
/// Lives here rather than in `ops/list_inference_profiles.rs` because every member it
/// carries is also a member `GetInferenceProfile` reports, and the two must agree about
/// them: a listed summary is the same profile the read of that one profile finds. It is
/// also the shape the published profiles are built in
/// ([`system_defined_inference_profiles`]), so a caller-created profile and a published one
/// are reported through ONE mapping.
#[must_use]
pub fn inference_profile_summary(profile: &InferenceProfileRecord) -> InferenceProfileSummary {
    InferenceProfileSummary {
        inference_profile_name: profile.name.clone(),
        description: profile.description.clone(),
        created_at: Some(date_time(profile.created_at)),
        updated_at: Some(date_time(profile.updated_at)),
        inference_profile_arn: profile.arn.clone(),
        models: profile.models.clone(),
        inference_profile_id: profile.id.clone(),
        status: profile.status.to_string(),
        r#type: profile.profile_type.to_string(),
    }
}

/// The system-defined inference profile Amazon Bedrock publishes in `region` under
/// `identifier`, if it publishes one.
///
/// `InferenceProfileIdentifier` admits the profile's own ARN or its bare
/// `inferenceProfileId` (`smithy_query(query="operation",operation="GetInferenceProfile")`),
/// so both spellings address the same published profile \u2014 resolved here the way
/// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::inference_profile_arn_of`]
/// resolves the two spellings of a profile a caller created. A published profile carries
/// the ARN of the region it is offered in, so one of another region names a profile of
/// another account-region.
#[must_use]
pub fn published_inference_profile(
    identifier: &str,
    region: &str,
) -> Option<InferenceProfileSummary> {
    system_defined_inference_profiles(region)
        .into_iter()
        .find(|published| {
            published.inference_profile_arn == identifier
                || published.inference_profile_id == identifier
        })
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
