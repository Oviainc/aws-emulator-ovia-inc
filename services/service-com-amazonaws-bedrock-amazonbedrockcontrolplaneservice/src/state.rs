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
//!
//! And it holds the build workflows of those policies —
//! `ops/start_automated_reasoning_policy_build_workflow.rs` starts them,
//! `ops/cancel_automated_reasoning_policy_build_workflow.rs` stops them and
//! `ops/get_automated_reasoning_policy_build_workflow.rs` reads them back — each with the
//! document the start named and the instant its non-terminal `status` is owed its move to
//! `COMPLETED`, or to `CANCELLED` once a cancel has asked for it.
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
    CustomModelSummary, CustomizationConfig, EndpointConfig, EvaluationConfig,
    ConverseSupport,
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
    /// `modelSourceIdentifier` — the Marketplace model this endpoint serves, as the create
    /// request named it and as each `RegisterMarketplaceModelEndpoint` re-states it: the
    /// registration is where the model deployed on the endpoint is recorded.
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
    /// When [`Self::status`] is owed its move to [`EVALUATION_JOB_STATUS_COMPLETED`], or
    /// `None` once the run has ended or a caller has stopped or deleted the job.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`EVALUATION_JOB_STATUS_STOPPED`], or
    /// `None` while no stop is in flight.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub stopped_due_at: Option<SystemTime>,
    /// When this job, which a batch delete accepted, is owed its removal from the store —
    /// and `None` while no delete has asked for one.
    ///
    /// The due INSTANT, not a timer, for the same reason [`Self::stopped_due_at`] is one.
    pub deleted_due_at: Option<SystemTime>,
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
    /// [`Self::settle`] is what materialises the one the clock owes. The completion the
    /// clock owed a running job is no longer owed once the run is being wound down, so it
    /// is dropped here.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = EVALUATION_JOB_STATUS_STOPPING;
        self.completed_due_at = None;
        self.stopped_due_at = Some(now + EVALUATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
    }

    /// Whether this job is one a batch delete may accept: its status is terminal.
    ///
    /// "An evaluation job can only be deleted if it has following status FAILED,
    /// COMPLETED, and STOPPED"
    /// (`smithy_query(query="operation",operation="BatchDeleteEvaluationJob")`), so a job
    /// in any other status — one still running, one winding down, one already being
    /// deleted — is not deletable yet.
    #[must_use]
    pub fn is_deletable(&self) -> bool {
        EVALUATION_JOB_DELETABLE_STATUSES.contains(&self.status)
    }

    /// Move this job to [`EVALUATION_JOB_STATUS_DELETING`] and record when its removal from
    /// the store is owed.
    ///
    /// The caller's own write, asked for by `BatchDeleteEvaluationJob` — not a clock
    /// transition: [`BedrockState::evaluation_job`] is what materialises the removal the
    /// clock owes.
    pub fn delete(&mut self, now: SystemTime) {
        self.status = EVALUATION_JOB_STATUS_DELETING;
        self.deleted_due_at = Some(now + EVALUATION_JOB_DELETED_AFTER);
        self.last_modified_at = now;
    }

    /// Whether this job's removal from the store is one the clock has already made due at
    /// `now`.
    #[must_use]
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.deleted_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the status move the clock has already made due at `now`: a running job's
    /// move to [`EVALUATION_JOB_STATUS_COMPLETED`], or a stopping job's to
    /// [`EVALUATION_JOB_STATUS_STOPPED`].
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its last modification. At most one of the two is pending,
    /// since a stop drops the completion the clock owed the run it wound down.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = EVALUATION_JOB_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = EVALUATION_JOB_STATUS_STOPPED;
            self.stopped_due_at = None;
            self.last_modified_at = due;
        }
    }
}

/// One stored advanced prompt optimization job: the identity
/// `CreateAdvancedPromptOptimizationJob` minted, the configuration that request carried,
/// the status it holds now and the instants a read reports.
///
/// `status` is `&'static str` for the same reason [`EvaluationJobRecord::status`] is: it is
/// a value this emulator derives rather than anything a request names.
#[derive(Debug, Clone)]
pub struct AdvancedPromptOptimizationJobRecord {
    /// The `AdvancedPromptOptimizationJobArn` this job is addressed by, minted by
    /// [`advanced_prompt_optimization_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so a bare-id lookup is per region without re-parsing an ARN, exactly as
    /// [`EvaluationJobRecord::region`] is.
    pub region: String,
    /// `jobDescription`, as the create request carried it (absent when it carried none).
    pub description: Option<String>,
    /// `inputConfig` — the S3 location of the JSONL input the job optimizes over.
    pub input_config: AdvancedPromptOptimizationInputConfig,
    /// `outputConfig` — the S3 prefix the optimization results are written under.
    pub output_config: AdvancedPromptOptimizationOutputConfig,
    /// `encryptionKeyArn`, as the create request carried it (absent when it carried none,
    /// in which case the output is encrypted with an Amazon-owned key and the read reports
    /// no key). Stored VERBATIM: the member is a `KmsKeyArn` in both the request and the
    /// response, so the ARN the request gave is the ARN the read reports.
    pub encryption_key_arn: Option<String>,
    /// `modelConfigurations` — the target models the job optimizes the prompts for, in the
    /// order the create request listed them.
    pub model_configurations: Vec<ModelConfiguration>,
    /// `tags` — the tags the create request attached.
    ///
    /// Kept with the record because it is part of the request that minted it; this
    /// resource's read operations report no tag member, so nothing reads them back, exactly
    /// as [`ProvisionedModelThroughputRecord::tags`] are kept.
    pub tags: Vec<Tag>,
    /// `AdvancedPromptOptimizationJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`], or `None` while no stop is in
    /// flight.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`EvaluationJobRecord::stopped_due_at`] is one.
    pub stopped_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed, which is the creation instant until something changes it.
    pub last_modified_at: SystemTime,
}

impl AdvancedPromptOptimizationJobRecord {
    /// Whether this job is one a stop may be asked for: it is still in progress.
    ///
    /// "Stops an advanced prompt optimization job that is in progress"
    /// (`smithy_query(query="doc",shape="StopAdvancedPromptOptimizationJob")`), so a job in
    /// any other status has nothing left to stop.
    #[must_use]
    pub fn is_in_progress(&self) -> bool {
        self.status == ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_IN_PROGRESS
    }

    /// Whether a stop has already taken this job over: it is winding down, or has wound
    /// down.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.status == ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING
            || self.status == ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED
    }

    /// Move this job to [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING`] and record
    /// when the stop is owed its move to
    /// [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopAdvancedPromptOptimizationJob` — not a
    /// clock transition: [`Self::settle`] is what materialises the one the clock owes.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING;
        self.stopped_due_at = Some(now + ADVANCED_PROMPT_OPTIMIZATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
    }

    /// Materialise the move to [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`] the
    /// clock has already made due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its last modification.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED;
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

/// One custom model deployment: the on-demand inference deployment
/// `CreateCustomModelDeployment` mints over a custom model and
/// `GetCustomModelDeployment` reads back.
#[derive(Debug, Clone)]
pub struct CustomModelDeploymentRecord {
    /// `customModelDeploymentArn` — the ARN this deployment is addressed by, and the key
    /// it is stored under ([`custom_model_deployment_arn`]).
    pub arn: String,
    /// `modelDeploymentName` — the name the create request gave it, unique within the
    /// account and region, and the OTHER spelling of a
    /// `CustomModelDeploymentIdentifier`.
    pub name: String,
    /// The region the create request addressed: a deployment belongs to one region, as the
    /// name's uniqueness does.
    pub region: String,
    /// `modelArn` — the custom model this deployment serves, as the create request carried
    /// it.
    pub model_arn: String,
    /// `description` — the create request's description, absent when it carried none.
    pub description: Option<String>,
    /// `tags` — the tags the create request attached.
    ///
    /// The ONE copy of this deployment's tags: the list the create filled is the list
    /// `ops/tag_resource.rs` rewrites and `ops/list_tags_for_resource.rs` reports, reached
    /// through [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::tagged_resource`].
    pub tags: Vec<Tag>,
    /// `status` — a [`CustomModelDeploymentStatus`](CUSTOM_MODEL_DEPLOYMENT_STATUS_CREATING).
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`], or `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub active_due_at: Option<SystemTime>,
    /// `createdAt` — when the create call accepted this deployment.
    pub created_at: SystemTime,
    /// `lastUpdatedAt` — when the deployment last changed, and `None` while nothing has
    /// changed it since it was created (the member is optional in the model).
    pub last_updated_at: Option<SystemTime>,
}

impl CustomModelDeploymentRecord {
    /// Materialise the move to [`CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`] the clock has
    /// already made due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the record reports as its last update.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.active_due_at.filter(|due| now >= *due) {
            self.status = CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE;
            self.active_due_at = None;
            self.last_updated_at = Some(due);
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
    /// `jobArn` — the `ModelCustomizationJobArn` of the customization job that PRODUCED
    /// this model, or `None` for a model that no job produced.
    ///
    /// "For models that you create with the CreateCustomModel API operation, this is NULL"
    /// (`smithy_query(query="operation",operation="GetCustomModel")`), so this is also what
    /// says which of the members a read reports are the JOB's: the job this names is where
    /// the hyperparameters, the datasets and the output location live, so there is only ever
    /// one copy of them.
    pub job_arn: Option<String>,
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

/// One stored model copy job: the identity `CreateModelCopyJob` minted, the source model
/// the request named, the copied model the job produces, and the job's own lifecycle
/// status.
///
/// `status` is `&'static str` for the same reason [`CustomModelRecord::status`] is: the
/// statuses are the ones this emulator's lifecycle passes through rather than anything a
/// request chooses.
#[derive(Debug, Clone)]
pub struct ModelCopyJobRecord {
    /// The `ModelCopyJobArn` this job is addressed by, minted by [`model_copy_job_arn`].
    pub arn: String,
    /// The region the create addressed — the region the copy is made INTO, which
    /// [`Self::arn`] also carries.
    pub region: String,
    /// `sourceModelArn`, as the create request spelled it. The model being copied lives in
    /// the region its own ARN names, which is how this operation copies across regions.
    pub source_model_arn: String,
    /// `sourceModelName` — the name of the custom model being copied, as that model
    /// carries it.
    pub source_model_name: Option<String>,
    /// `sourceAccountId` — the account the model being copied originated from, read off the
    /// source ARN.
    pub source_account_id: String,
    /// `targetModelArn` — the `CustomModelArn` the copied model is minted under, decided
    /// when the job is accepted so the job reports the model it will produce from the
    /// start.
    pub target_model_arn: String,
    /// `targetModelName`, as the create request spelled it.
    pub target_model_name: String,
    /// `targetModelKmsKeyArn` — the key the copy is encrypted with (absent when the create
    /// named none).
    pub target_model_kms_key_arn: Option<String>,
    /// `targetModelTags` — the tags the create attached to the copied model.
    pub target_model_tags: Vec<Tag>,
    /// `ModelCopyJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`MODEL_COPY_JOB_STATUS_COMPLETED`], or
    /// `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`CustomModelRecord::active_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
}

/// One stored model customization job: the identity `CreateModelCustomizationJob` minted,
/// the request that minted it, the custom model it is customizing, and the job's own
/// lifecycle status.
///
/// `status` and `customization_type` are `&'static str` for the same reason
/// [`ModelImportJobRecord::status`] is: the statuses are the ones this emulator's lifecycle
/// passes through, and the customization type is resolved against the modelled enum
/// ([`customization_type_of`]) rather than kept as whatever text arrived.
#[derive(Debug, Clone)]
pub struct ModelCustomizationJobRecord {
    /// The `ModelCustomizationJobArn` this job is addressed by, minted by
    /// [`model_customization_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it — the other spelling a
    /// `ModelCustomizationJobIdentifier` may carry.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index and the uniqueness checks are per region without re-parsing an
    /// ARN, exactly as [`ModelImportJobRecord::region`] is.
    pub region: String,
    /// `clientRequestToken`, as the create request carried it — a member the read reports
    /// back.
    pub client_request_token: Option<String>,
    /// `roleArn` — the service role Amazon Bedrock assumes to read the training data and
    /// write the model artifacts.
    pub role_arn: String,
    /// `customModelName` — the name of the model this job is customizing, which the read
    /// reports as its `outputModelName`.
    pub output_model_name: String,
    /// The `CustomModelArn` of the model this job is customizing, minted by
    /// [`custom_model_arn`] when the job is accepted: it is the job's target, so it is
    /// fixed from that call onwards, exactly as [`ModelImportJobRecord::imported_model_arn`]
    /// is.
    pub output_model_arn: String,
    /// `baseModelArn` — the `FoundationModelArn` of the base model being customized, as
    /// [`foundation_model_arn`] spells it for the catalog row the request's
    /// `baseModelIdentifier` resolved to.
    pub base_model_arn: String,
    /// `CustomizationType` — the customization this job runs.
    pub customization_type: &'static str,
    /// The `outputModelKmsKeyArn` a read reports for the request's `customModelKmsKeyId`,
    /// as [`kms_key_arn`] completes it.
    pub output_model_kms_key_arn: Option<String>,
    /// `hyperParameters`, as the create request carried them (absent when it carried none).
    pub hyper_parameters: Option<HashMap<String, String>>,
    /// `trainingDataConfig` — where the training data is read from.
    pub training_data_config: TrainingDataConfig,
    /// `validationDataConfig` — the validators the job runs.
    ///
    /// Stored as the shape rather than as an `Option` of it: the member is optional in the
    /// request and REQUIRED in the read
    /// (`smithy_query(query="operation",operation="GetModelCustomizationJob")`), and its
    /// own `validators` list admits zero entries, so a create that named no validation data
    /// is the job with no validators.
    pub validation_data_config: ValidationDataConfig,
    /// `outputDataConfig` — where the job writes its output.
    pub output_data_config: OutputDataConfig,
    /// `vpcConfig`, as the create request carried it (absent when it carried none).
    pub vpc_config: Option<VpcConfig>,
    /// `customizationConfig`, as the create request carried it (absent when it carried
    /// none).
    pub customization_config: Option<CustomizationConfig>,
    /// `jobTags` — the tags attached to the job itself.
    ///
    /// Kept for the reason [`ProvisionedModelThroughputRecord::tags`] is: it is part of the
    /// request that minted the record, and the job's own read reports no tag member.
    pub job_tags: Vec<Tag>,
    /// `customModelTags` — the tags attached to the model this job is customizing, which
    /// become that model's tags when the job completes.
    pub custom_model_tags: Vec<Tag>,
    /// `ModelCustomizationJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED`], or `None` once the job is terminal or
    /// a stop has been asked for.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED`], or `None` while no stop is in flight.
    pub stopped_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed: the creation instant until something changes it, and the
    /// instant a transition was owed once the status has moved.
    pub last_modified_at: SystemTime,
    /// When the job reached a terminal status, or `None` while it is still running.
    pub ended_at: Option<SystemTime>,
}

impl ModelCustomizationJobRecord {
    /// Whether this job is still running, so a stop has something to wind down.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.status == MODEL_CUSTOMIZATION_JOB_STATUS_IN_PROGRESS
    }

    /// Whether a stop has already taken this job over: it is winding down, or it has
    /// wound down.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.status == MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING
            || self.status == MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED
    }

    /// Move this job to [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING`] and record when the
    /// stop is owed its move to [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopModelCustomizationJob` — not a clock
    /// transition. The training it was running is being wound down, so the job is owed no
    /// completion any more and mints no custom model.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING;
        self.stopped_due_at = Some(now + MODEL_CUSTOMIZATION_JOB_STOPPED_AFTER);
        self.completed_due_at = None;
        self.last_modified_at = now;
    }
}

/// `ModelInvocationJobStatus` — a batch inference job that has been submitted: "Submitted
/// – This job has been submitted to a queue for validation"
/// (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`). The
/// status every job this emulator mints carries from the call that submits it until the
/// clock moves it on.
pub const MODEL_INVOCATION_JOB_STATUS_SUBMITTED: &str = "Submitted";

/// `ModelInvocationJobStatus` — a batch inference job being checked over before it is
/// queued to run: "Validating – This job is being validated for the requirements described
/// in Format and upload your batch inference data".
pub const MODEL_INVOCATION_JOB_STATUS_VALIDATING: &str = "Validating";

/// `ModelInvocationJobStatus` — a validated batch inference job waiting its turn:
/// "Scheduled – This job has been validated and is now in a queue. The job will
/// automatically start when it reaches its turn".
pub const MODEL_INVOCATION_JOB_STATUS_SCHEDULED: &str = "Scheduled";

/// `ModelInvocationJobStatus` — a batch inference job that has begun: "InProgress – This
/// job has begun. You can start viewing the results in the output S3 location".
pub const MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job that ran to
/// the end of its input: "Completed – This job has successfully completed".
pub const MODEL_INVOCATION_JOB_STATUS_COMPLETED: &str = "Completed";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job some but not
/// all of whose records were processed: "PartiallyCompleted – This job has partially
/// completed. Not all of your records could be processed in time. View the output files in
/// the output S3 location".
pub const MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED: &str = "PartiallyCompleted";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job that could not
/// be run: "Failed – This job has failed. Check the failure message for any further
/// details". The failure message is [`ModelInvocationJobRecord::message`].
pub const MODEL_INVOCATION_JOB_STATUS_FAILED: &str = "Failed";

/// `ModelInvocationType` — the Converse endpoint, one of the two values the modelled enum
/// carries (`crate::ops::types::MODEL_INVOCATION_TYPE_VALUES`). Named because the job's
/// work step invokes each record through the endpoint the submission chose.
pub const MODEL_INVOCATION_TYPE_CONVERSE: &str = "Converse";

/// `ModelInvocationJobStatus` — a batch inference job a user asked to stop, while that
/// stop is still being carried out: "Stopping – This job is being stopped by a user".
pub const MODEL_INVOCATION_JOB_STATUS_STOPPING: &str = "Stopping";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job a user
/// stopped: "Stopped – This job was stopped by a user".
pub const MODEL_INVOCATION_JOB_STATUS_STOPPED: &str = "Stopped";

/// The `ModelInvocationJobStatus` values a batch inference job can still be stopped from:
/// a job that has been submitted, is being validated, is queued or is running has work a
/// stop winds down (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`).
/// Every other value of the enum is either terminal or a stop already in flight, which is
/// why a stop of one of those changes nothing.
pub const MODEL_INVOCATION_JOB_STOPPABLE_STATUSES: &[&str] = &[
    MODEL_INVOCATION_JOB_STATUS_SUBMITTED,
    MODEL_INVOCATION_JOB_STATUS_VALIDATING,
    MODEL_INVOCATION_JOB_STATUS_SCHEDULED,
    MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS,
];

/// How long after `StopModelInvocationJob` accepted the stop a batch inference job is owed
/// its move from [`MODEL_INVOCATION_JOB_STATUS_STOPPING`] to
/// [`MODEL_INVOCATION_JOB_STATUS_STOPPED`]. Chosen for the reason
/// [`MODEL_INVOCATION_JOB_BEGINS_AFTER`] is, and non-zero so a caller reading the job
/// straight after the stop still sees the status the stop recorded.
pub const MODEL_INVOCATION_JOB_STOPPED_AFTER: Duration = Duration::from_millis(500);

/// How long after it was submitted a batch inference job is owed its move from
/// [`MODEL_INVOCATION_JOB_STATUS_SUBMITTED`] to
/// [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`].
///
/// The emulator's own span, chosen like [`IMPORT_JOB_COMPLETE_AFTER`] rather than read off
/// any published figure: the queueing a real submission waits through is not something this
/// emulator performs, and the span is non-zero so a caller reading the job straight after
/// the create still sees the status the create answered with.
pub const MODEL_INVOCATION_JOB_VALIDATES_AFTER: Duration = Duration::from_millis(500);

/// How long after its validation began a batch inference job is owed its move from
/// [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`] to
/// [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`]. Chosen, and non-zero, for the reason
/// [`MODEL_INVOCATION_JOB_VALIDATES_AFTER`] is: every pre-start status the model documents
/// is one a caller can read the job in rather than one it is carried past.
pub const MODEL_INVOCATION_JOB_SCHEDULES_AFTER: Duration = Duration::from_millis(500);

/// How long after it was queued for its turn a batch inference job is owed its move from
/// [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`] to
/// [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`]. Chosen for the same reason
/// [`MODEL_INVOCATION_JOB_VALIDATES_AFTER`] is.
pub const MODEL_INVOCATION_JOB_BEGINS_AFTER: Duration = Duration::from_millis(500);

/// How long a running batch inference job dwells in
/// [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`] before it is owed its move to
/// [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`].
///
/// CHOSEN, not documented: a real submission runs for as long as its records and the queue
/// it is in take, which is no published figure, so this is the emulator's own span. It is
/// counted from the instant the job was owed its start
/// ([`ModelInvocationJobClock::armed_at`]), so the whole progression stands on the
/// SUBMISSION's own timeline and the elapsed time since the submission — not the number of
/// times anybody read the job — is what carries it to its terminal status.
///
/// It is LONGER than the three spans before it, because this is the one status of the
/// progression a caller is documented to act on while it holds: "InProgress – this job has
/// begun. You can start viewing the results in the output S3 location", and it is the last
/// status a stop still has work to wind down from
/// ([`MODEL_INVOCATION_JOB_STOPPABLE_STATUSES`]). A client of a job the service documents as
/// long-running polls it at intervals of seconds to tens of seconds, so a running window
/// shorter than that would be one no such client ever reads the job in — and then the
/// progress it watches and the stop it issues on a running job would both be unreachable.
/// The window is of that order and no larger: the terminal status, the record counts and the
/// results under the output prefix are still reached inside one short polling pass rather
/// than after a wall-clock wait of the size a caller budgets for the whole interaction.
///
/// The WORK the span stands over is performed, not skipped
/// ([`model_invocation_run::run_batch_inference`] reads the input and invokes the model once
/// per record), and the span does not scale with the input: it is materialised from the
/// stored due instant on the next call that touches the record rather than waited on, so no
/// handler sleeps.
pub const MODEL_INVOCATION_JOB_COMPLETE_AFTER: Duration = Duration::from_secs(12);

/// The EARLIEST instants a batch inference job submitted at a given moment is owed each of
/// its status transitions at: the whole progression
/// `Submitted -> Validating -> Scheduled -> InProgress -> Completed`, each stage standing its
/// own span after the one before it.
///
/// The submission arms them all and nothing re-counts them afterwards
/// ([`ModelInvocationJobRecord::settle`]), so these are the instants every caller reads the
/// job's stages at, whether it polls once or a hundred times: a stage whose instant has
/// elapsed is behind the job for everybody.
#[derive(Debug, Clone, Copy)]
pub struct ModelInvocationJobClock {
    /// When [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`] is owed.
    pub validating_at: SystemTime,
    /// When [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`] is owed.
    pub scheduled_at: SystemTime,
    /// When [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`] is owed.
    pub in_progress_at: SystemTime,
    /// When [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`] is owed.
    pub completed_at: SystemTime,
}

impl ModelInvocationJobClock {
    /// The clock of a job submitted at `submit_time`: each transition stands its own span
    /// after the transition before it.
    ///
    /// The instants are strictly increasing, so the progression is walked in order whatever
    /// instant a caller observes it at: a job read exactly as one stage falls due holds that
    /// stage, and one first read long after the submission is carried through every stage
    /// whose instant has already passed, to the status the elapsed time owes it.
    #[must_use]
    pub fn armed_at(submit_time: SystemTime) -> Self {
        let validating_at = submit_time + MODEL_INVOCATION_JOB_VALIDATES_AFTER;
        let scheduled_at = validating_at + MODEL_INVOCATION_JOB_SCHEDULES_AFTER;
        let in_progress_at = scheduled_at + MODEL_INVOCATION_JOB_BEGINS_AFTER;
        let completed_at = in_progress_at + MODEL_INVOCATION_JOB_COMPLETE_AFTER;
        Self {
            validating_at,
            scheduled_at,
            in_progress_at,
            completed_at,
        }
    }
}

/// The seconds in the hour a `timeoutDurationInHours` counts, so the instant a job times
/// out is computed on one scale wherever it is needed.
pub const SECONDS_PER_HOUR: u64 = 60 * 60;

/// The most batch inference jobs this account may hold at once, across every region it has
/// submitted one in, past which a create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota, for the reason
/// [`MAX_MODEL_CUSTOMIZATION_JOBS_PER_ACCOUNT`] is one: without a server-side cap a caller
/// could grow this part of the store with nothing but further submissions.
pub const MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT: usize = 100;

/// One batch inference job, as `CreateModelInvocationJob` submitted it and
/// `GetModelInvocationJob` and `ListModelInvocationJobs` report it.
///
/// `status` and `model_invocation_type` are `&'static str` for the reason
/// [`ModelCustomizationJobRecord::status`] is: the statuses are the ones this emulator's
/// lifecycle passes through, and the invocation type is resolved against the modelled enum
/// ([`model_invocation_type_of`]) rather than kept as whatever text arrived.
#[derive(Debug, Clone)]
pub struct ModelInvocationJobRecord {
    /// The `ModelInvocationJobArn` this job is addressed by, minted by
    /// [`model_invocation_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it. NOT an identifier: a
    /// `ModelInvocationJobIdentifier` is the job ARN or the minted job id alone
    /// (`smithy_query(query="operation",operation="GetModelInvocationJob")`), so a name is
    /// reported and filtered on rather than resolved through.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it so
    /// a listing is per region without re-parsing an ARN, exactly as
    /// [`ModelCustomizationJobRecord::region`] is.
    pub region: String,
    /// `clientRequestToken`, as the create request carried it — a member both reads report
    /// back.
    pub client_request_token: Option<String>,
    /// `roleArn` — the service role Amazon Bedrock assumes to read the input and write the
    /// output of this job.
    pub role_arn: String,
    /// `modelId`, as the create request spelled it: the identifier admits a bare model id,
    /// a foundation model ARN, a custom model ARN and an inference profile ARN, and the
    /// reads report the spelling the submission carried.
    pub model_id: String,
    /// `inputDataConfig` — where the job reads its records from.
    pub input_data_config: ModelInvocationJobInputDataConfig,
    /// `outputDataConfig` — where the job writes its records to.
    pub output_data_config: ModelInvocationJobOutputDataConfig,
    /// `vpcConfig`, as the create request carried it (absent when it carried none).
    pub vpc_config: Option<VpcConfig>,
    /// `timeoutDurationInHours`, as the create request carried it (absent when it carried
    /// none, which is a job with no timeout to report).
    pub timeout_duration_in_hours: Option<i32>,
    /// `jobExpirationTime` — "the time at which the batch inference job times or timed
    /// out": [`Self::submit_time`] plus [`Self::timeout_duration_in_hours`], and `None` for
    /// a job the request set no timeout for.
    pub job_expiration_time: Option<SystemTime>,
    /// `ModelInvocationType` — the invocation endpoint this job runs against.
    pub model_invocation_type: &'static str,
    /// `tags` — the tags the submission attached to the job.
    ///
    /// Kept for the reason [`ProvisionedModelThroughputRecord::tags`] is: it is part of the
    /// request that minted the record, and the job's own reads report no tag member.
    pub tags: Vec<Tag>,
    /// `ModelInvocationJobStatus`.
    pub status: &'static str,
    /// The terminal `ModelInvocationJobStatus` this job's [`Self::completed_due_at`] moves
    /// it to: [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`] for a job whose every record was
    /// processed, [`MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED`] for one some of whose
    /// records could not be, and [`MODEL_INVOCATION_JOB_STATUS_FAILED`] for one that could
    /// not be run at all.
    ///
    /// Decided by the WORK, not by the clock: the submission's validation of the input
    /// ([`model_invocation_run::read_input_records`]) and the run over its records
    /// ([`model_invocation_run::run_batch_inference`]) are what set it, so the status a job
    /// ends in reports what happened to its records.
    pub terminal_status: &'static str,
    /// `message` — "if the batch inference job failed, this field contains a message
    /// describing why the job failed", and `None` for a job that has not failed.
    pub message: Option<String>,
    /// `totalRecordCount` — "the total number of records in the batch inference job":
    /// the records read out of the input object this job was pointed at.
    ///
    /// `None` only while nothing is known about that object: an emulator assembled with no
    /// object store to ask establishes neither its records nor its absence
    /// ([`model_invocation_run::InputVerdict::Unknown`]), and a count is reported only
    /// where one was counted.
    pub total_record_count: Option<i64>,
    /// `processedRecordCount` — "the number of records that have been processed in the
    /// batch inference job": [`Self::success_record_count`] and
    /// [`Self::error_record_count`] together, zero until the job's run materialises.
    pub processed_record_count: i64,
    /// `successRecordCount` — "the number of records that were successfully processed in
    /// the batch inference job": the records the model answered for.
    pub success_record_count: i64,
    /// `errorRecordCount` — "the number of records that failed to process in the batch
    /// inference job": the records the model could not be invoked for.
    pub error_record_count: i64,
    /// Whether this job's run — reading its records, invoking the model over each of them
    /// and storing the results under its output prefix — is still owed.
    ///
    /// The run is owed from the submission that validated an input, falls due with the
    /// status that says the job has begun, and is taken ONCE ([`Self::due_run`]): a run the
    /// object store did not take is given back, so a later call takes it again, and no
    /// later call runs a job whose results are already stored.
    pub run_owed: bool,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`], or `None` once that move has been
    /// materialised.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one. It and the three below are the
    /// job's whole clock, armed by the submission ([`ModelInvocationJobClock::armed_at`]) and
    /// only ever spent — never re-counted — by the calls that observe the job
    /// ([`ModelInvocationJobRecord::settle`]), so the progression a caller reads is the one
    /// the time since the submission owes it.
    pub validating_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`], or `None` once that move has been
    /// materialised.
    pub scheduled_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`], or `None` once the job has begun.
    pub in_progress_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`], or `None` once the job has reached a
    /// terminal status.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_STOPPED`], or `None` while no stop is in flight.
    pub stopped_due_at: Option<SystemTime>,
    /// `submitTime` — when the job was submitted.
    pub submit_time: SystemTime,
    /// `lastModifiedTime`: the submission instant until something changes the job, and the
    /// instant a transition was owed once the status has moved.
    pub last_modified_at: SystemTime,
    /// `endTime` — when the job reached a terminal status, or `None` while it has not.
    pub ended_at: Option<SystemTime>,
}

impl ModelInvocationJobRecord {
    /// Whether a stop has work to wind down on this job: its status is one a job that has
    /// not finished carries ([`MODEL_INVOCATION_JOB_STOPPABLE_STATUSES`]).
    #[must_use]
    pub fn is_stoppable(&self) -> bool {
        MODEL_INVOCATION_JOB_STOPPABLE_STATUSES.contains(&self.status)
    }

    /// Move this job to [`MODEL_INVOCATION_JOB_STATUS_STOPPING`] and record when the stop
    /// is owed its move to [`MODEL_INVOCATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopModelInvocationJob` — not a clock
    /// transition. The inference it was queued for or running is being wound down, so none
    /// of the transitions the submission armed ([`ModelInvocationJobClock::armed_at`]) is
    /// owed any more.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = MODEL_INVOCATION_JOB_STATUS_STOPPING;
        self.validating_due_at = None;
        self.scheduled_due_at = None;
        self.in_progress_due_at = None;
        self.completed_due_at = None;
        self.stopped_due_at = Some(now + MODEL_INVOCATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
        // The run it was queued for is being wound down, so the records it would have
        // processed are not processed and nothing is written for them.
        self.run_owed = false;
    }

    /// Whether the job has BEGUN: its status is one the model documents the results of the
    /// run under — "InProgress – this job has begun. You can start viewing the results in
    /// the output S3 location", and the terminal statuses that say to view those files.
    #[must_use]
    pub fn has_begun(&self) -> bool {
        matches!(
            self.status,
            MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS
                | MODEL_INVOCATION_JOB_STATUS_COMPLETED
                | MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED
        )
    }

    /// The run this job still owes, taken ONCE — or `None` when it owes none yet.
    ///
    /// The work is materialised exactly like the status transition that started it: it fell
    /// due when the clock moved the job into the status that says it has begun, whether or
    /// not anybody called, so the first operation that finds the job begun takes the run
    /// ([`model_invocation_run::run_batch_inference`]) and no later call takes it again —
    /// which is what keeps a read reporting the same counts and leaves what a client
    /// fetched out of the output location as it fetched it.
    ///
    /// A job nothing is known about the input of owes nothing: there are no records to run
    /// over ([`Self::total_record_count`]).
    pub fn due_run(&mut self) -> Option<model_invocation_run::DueRun> {
        if !self.run_owed || !self.has_begun() || self.total_record_count.is_none() {
            return None;
        }
        self.run_owed = false;
        Some(model_invocation_run::DueRun {
            arn: self.arn.clone(),
            region: self.region.clone(),
            model_id: self.model_id.clone(),
            invocation_type: self.model_invocation_type,
            input_uri: model_invocation_run::input_s3_uri(&self.input_data_config).to_string(),
            output_uri: model_invocation_run::output_s3_uri(&self.output_data_config).to_string(),
        })
    }

    /// Write back what the run established, and give the run back when it established
    /// nothing.
    ///
    /// The counts are the run's own tally over the records it read, so what the two reads
    /// report accounts for the results stored under the output prefix. A run that could not
    /// be performed at all leaves the job still owing it, so a later call takes it again
    /// rather than reporting a job that did nothing as finished.
    ///
    /// The terminal status this job ends in is decided here; a job the clock has ALREADY
    /// carried into its terminal status is moved to the one the run decided, so no job
    /// reports `Completed` with records that failed.
    pub fn record_run(&mut self, outcome: model_invocation_run::RunOutcome) {
        match outcome {
            model_invocation_run::RunOutcome::Ran {
                success,
                error,
                message,
            } => {
                self.success_record_count = success;
                self.error_record_count = error;
                self.processed_record_count = success.saturating_add(error);
                self.terminal_status = if error == 0 {
                    MODEL_INVOCATION_JOB_STATUS_COMPLETED
                } else if success == 0 {
                    MODEL_INVOCATION_JOB_STATUS_FAILED
                } else {
                    MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED
                };
                self.message = message;
            }
            model_invocation_run::RunOutcome::Failed(message) => {
                self.terminal_status = MODEL_INVOCATION_JOB_STATUS_FAILED;
                self.message = Some(message);
            }
            model_invocation_run::RunOutcome::Deferred => {
                self.run_owed = true;
                return;
            }
        }
        if self.has_begun() && self.completed_due_at.is_none() {
            self.status = self.terminal_status;
        }
    }

    /// Materialise EVERY status transition the clock has already made due at `now`.
    ///
    /// The progression runs on the SUBMISSION's own timeline: the submission fixed the
    /// instant of each stage ([`ModelInvocationJobClock::armed_at`]) and nothing re-counts
    /// them, so what a job's status is at `now` is decided by the time elapsed since it was
    /// submitted and by nothing else — not by how many times, or how often, anybody read it.
    /// A job read once, long enough after its submission, is therefore answered with the
    /// status that elapsed time owes it, which for a job whose run has fallen due is a
    /// terminal one; a job polled tightly observes the intermediate stages on the way there,
    /// because each stage still dwells its own span. Intermediate statuses are observable,
    /// never waypoints a caller has to poll through.
    ///
    /// The stages are SEQUENTIAL and their instants strictly increasing, so the walk stops at
    /// the first stage still ahead of `now` and never materialises a later stage before an
    /// earlier one. The stop a caller asked for is owed the same way, from the instant
    /// [`Self::stop`] recorded.
    ///
    /// No move is materialised before its span has elapsed, which is why the instant it was
    /// owed at — not `now` — is the one the job reports as its last modification and its
    /// end, and why settling twice over reports the same instants.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.validating_due_at {
            if now < due {
                return;
            }
            self.status = MODEL_INVOCATION_JOB_STATUS_VALIDATING;
            self.validating_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.scheduled_due_at {
            if now < due {
                return;
            }
            self.status = MODEL_INVOCATION_JOB_STATUS_SCHEDULED;
            self.scheduled_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.in_progress_due_at {
            if now < due {
                return;
            }
            self.status = MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS;
            self.in_progress_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.completed_due_at {
            if now < due {
                return;
            }
            // The terminal status is the one the WORK decided ([`Self::terminal_status`]): a
            // job every record of which was processed completed, one some of whose records
            // could not be processed completed in part, and one that could not be run at all
            // failed with the message it failed on. The run itself is taken by the same call
            // that materialises this ([`Self::due_run`]), so the counts and the objects under
            // the output prefix are there for the read that first reports a terminal status.
            self.status = self.terminal_status;
            self.completed_due_at = None;
            self.ended_at = Some(due);
            self.last_modified_at = due;
            return;
        }
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = MODEL_INVOCATION_JOB_STATUS_STOPPED;
            self.stopped_due_at = None;
            self.ended_at = Some(due);
            self.last_modified_at = due;
        }
    }
}

/// The most Automated Reasoning policies this account may hold at once, across every
/// region it has created one in, past which a create is refused with
/// `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota: without a server-side cap a
/// caller could grow this part of the store with nothing but fresh policy names, or with
/// fresh regions, which is why the bound is over the whole store rather than per region.
pub const MAX_AUTOMATED_REASONING_POLICIES_PER_ACCOUNT: usize = 100;

/// `AutomatedReasoningPolicyDefinition$version` when the request carried no version of
/// its own — the member's modelled `@default("1")`
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyDefinition")`).
pub const POLICY_DEFINITION_FORMAT_VERSION: &str = "1";

/// The most build workflows ONE Automated Reasoning policy may hold at once, past which a
/// start is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota, and it is the length the
/// listing that reports them is modelled with: `ListAutomatedReasoningPolicyBuildWorkflows`
/// answers an `AutomatedReasoningPolicyBuildWorkflowSummaries` of `@length(min: 0, max:
/// 1000)`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyBuildWorkflows")`),
/// so a policy that held more could not report them all. Without a server-side cap a
/// caller could also grow this part of the store with nothing but fresh starts.
pub const MAX_BUILD_WORKFLOWS_PER_POLICY: usize = 1000;

/// How long after `StartAutomatedReasoningPolicyBuildWorkflow` accepts a build workflow
/// its `status` is owed its move to [`BUILD_WORKFLOW_STATUS_COMPLETED`].
///
/// The emulator's own span, chosen like [`IMPORT_JOB_COMPLETE_AFTER`] rather than
/// documented by AWS: a workflow AWS answers in a non-terminal status is owed a
/// transition, and the span is non-zero so a caller reading straight after the start still
/// sees the status the start answered.
pub const BUILD_WORKFLOW_COMPLETE_AFTER: Duration = Duration::from_millis(500);

/// `AutomatedReasoningPolicyBuildWorkflowStatus$SCHEDULED` — the status a build workflow a
/// start has just accepted carries, before the clock owes it its completion.
pub const BUILD_WORKFLOW_STATUS_SCHEDULED: &str = "SCHEDULED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$COMPLETED` — the terminal status the clock
/// owes a build workflow [`BUILD_WORKFLOW_COMPLETE_AFTER`] from its start.
pub const BUILD_WORKFLOW_STATUS_COMPLETED: &str = "COMPLETED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$CANCEL_REQUESTED` — the status a build
/// workflow carries from the `CancelAutomatedReasoningPolicyBuildWorkflow` that asked for
/// its cancellation until the clock owes it [`BUILD_WORKFLOW_STATUS_CANCELLED`]. The enum
/// carries this status beside the terminal one because stopping a running build is
/// asynchronous: the ask is recorded first and the workflow winds down afterwards.
pub const BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED: &str = "CANCEL_REQUESTED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$CANCELLED` — the terminal status the clock
/// owes a build workflow whose cancellation was asked for.
pub const BUILD_WORKFLOW_STATUS_CANCELLED: &str = "CANCELLED";

/// `AutomatedReasoningPolicyBuildWorkflowStatus$FAILED` — the other terminal status the
/// enum admits. Nothing in this emulator moves a workflow to it, and it is named here so
/// [`AutomatedReasoningPolicyBuildWorkflowRecord::is_active`] decides on the whole set of
/// terminal statuses rather than on the two this emulator produces.
pub const BUILD_WORKFLOW_STATUS_FAILED: &str = "FAILED";

/// How long after `CancelAutomatedReasoningPolicyBuildWorkflow` accepts a cancellation the
/// workflow's `status` is owed its move from [`BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED`] to
/// [`BUILD_WORKFLOW_STATUS_CANCELLED`].
///
/// The emulator's own span, chosen like [`BUILD_WORKFLOW_COMPLETE_AFTER`] rather than
/// documented by AWS, and non-zero so a caller reading straight after the cancel still
/// sees the status that cancel recorded.
pub const BUILD_WORKFLOW_CANCELLED_AFTER: Duration = Duration::from_millis(500);

/// What an operation ADDRESSED BY a `buildWorkflowId` — the read of a build workflow, its
/// cancel, its delete — answers as its `ResourceNotFoundException` message when this store
/// holds no such workflow.
///
/// RECORDED behaviour, and a message with NO variable parts: Amazon Bedrock names neither
/// the policy the request addressed nor the workflow id it carried, so the wording is the
/// whole requirement and is shared by every operation that ADDRESSES the resource.
///
/// The TEST surface words the same miss differently
/// ([`REQUEST_BUILD_WORKFLOW_NOT_FOUND`]), as the two guardrail sentences differ
/// ([`GUARDRAIL_NOT_FOUND`] against [`REQUEST_GUARDRAIL_NOT_FOUND`]): a resource a request
/// is addressed by and a resource a request NAMES as the subject of other work are reported
/// by different parts of the service, and each has its own recorded sentence.
#[must_use]
pub fn build_workflow_not_found_message() -> &'static str {
    "Build WorkflowExecution not found"
}

/// What the TEST surface — `StartAutomatedReasoningPolicyTestWorkflow` and
/// `GetAutomatedReasoningPolicyTestResult`, whose requests NAME the build workflow the tests
/// are run against — answers as its `ResourceNotFoundException` message when the named
/// workflow is not there.
///
/// RECORDED behaviour, and wholly FIXED: Amazon Bedrock names the resource in the words its
/// own API spells it with and carries neither the policy ARN nor the id the request sent. It
/// is a different sentence from the one an operation addressed BY a workflow id answers
/// ([`build_workflow_not_found_message`]) — the same split the guardrail has — so a caller
/// learns which half of its request named the missing workflow.
pub const REQUEST_BUILD_WORKFLOW_NOT_FOUND: &str = "Build workflow not found";

/// What a write to a build workflow whose `lastUpdatedAt` concurrency token is not the one
/// the stored workflow holds answers as its `ConflictException` message, for the write named
/// by `action` ("deleting").
///
/// Worded as this service words that error class for a child resource of an Automated
/// Reasoning policy ([`automated_reasoning_policy_test_case_conflict_message`]): the
/// conflict, the write it conflicts with, the resource, and then the id and the policy ARN
/// the REQUEST carried — so the wording is fixed and no instant appears in it, neither the
/// one the workflow holds nor the one the request submitted.
///
/// The resource NAME here is the one this conflict refusal carries, and it is not derived
/// from [`build_workflow_not_found_message`]: the two refusals of this resource are worded
/// independently, each from its own recording, so the noun in one is no evidence about the
/// other.
#[must_use]
pub fn automated_reasoning_policy_build_workflow_conflict_message(
    action: &str,
    build_workflow_id: &str,
    policy_arn: &str,
) -> String {
    format!(
        "Conflict with {action} Build WorkflowExecution with buildWorkflowId: \
         {build_workflow_id} and policyArn: {policy_arn}"
    )
}

/// One stored Automated Reasoning policy build workflow: the identity
/// `StartAutomatedReasoningPolicyBuildWorkflow` minted, the document that request named,
/// and the status `GetAutomatedReasoningPolicyBuildWorkflow` reports.
///
/// The source DOCUMENTS themselves are not kept: a build workflow's reads report only a
/// document's name, content type and description
/// (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyBuildWorkflow")`),
/// so holding the up-to-5-MB blob a start carried would grow the store with bytes no
/// operation reads.
///
/// `status` is a `&'static str` for the reason [`ModelImportJobRecord::status`] is: it is a
/// value this emulator derives rather than one a request names.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyBuildWorkflowRecord {
    /// The `AutomatedReasoningPolicyArn` of the policy this workflow builds — the first
    /// half of the key this family's store is keyed by.
    pub policy_arn: String,
    /// `buildWorkflowId` — minted by
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::mint_build_workflow_id`],
    /// and the second half of that key.
    pub id: String,
    /// `buildWorkflowType`, as the start request named it
    /// (`AutomatedReasoningPolicyBuildWorkflowType`).
    pub workflow_type: String,
    /// `AutomatedReasoningPolicyBuildWorkflowStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`BUILD_WORKFLOW_STATUS_COMPLETED`], or
    /// `None` once it is terminal or a cancellation has been asked for.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`BUILD_WORKFLOW_STATUS_CANCELLED`], or
    /// `None` while no cancellation is in flight.
    pub cancelled_due_at: Option<SystemTime>,
    /// `documentName` of the source document the start carried, or `None` when it carried
    /// no document at all.
    pub document_name: Option<String>,
    /// `documentContentType` of that same document.
    pub document_content_type: Option<String>,
    /// `documentDescription` of that same document, absent when the document carried none.
    pub document_description: Option<String>,
    pub created_at: SystemTime,
    /// When the workflow last changed: the start instant until the clock moves its status.
    pub updated_at: SystemTime,
}

impl AutomatedReasoningPolicyBuildWorkflowRecord {
    /// Whether this workflow is still working, so the policy it builds is in use by it.
    ///
    /// A workflow whose cancellation has been asked for is still winding down, so it is
    /// still working: only a terminal status frees the policy it builds.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(
            self.status,
            BUILD_WORKFLOW_STATUS_COMPLETED
                | BUILD_WORKFLOW_STATUS_CANCELLED
                | BUILD_WORKFLOW_STATUS_FAILED
        )
    }

    /// Whether a cancellation has already taken this workflow over: it is winding down, or
    /// it has wound down.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.status == BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED
            || self.status == BUILD_WORKFLOW_STATUS_CANCELLED
    }

    /// Materialise the status transition the clock has already made due at `now`: the
    /// completion a start owes a workflow, or the cancellation a cancel owes one.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the workflow reports as its last update. The two instants are mutually
    /// exclusive: a cancel drops the completion it was owed.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = BUILD_WORKFLOW_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.updated_at = due;
        }
        if let Some(due) = self.cancelled_due_at.filter(|due| now >= *due) {
            self.status = BUILD_WORKFLOW_STATUS_CANCELLED;
            self.cancelled_due_at = None;
            self.updated_at = due;
        }
    }

    /// Move this workflow to [`BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED`] and record when it
    /// is owed its move to [`BUILD_WORKFLOW_STATUS_CANCELLED`].
    ///
    /// The caller's own write, asked for by
    /// `CancelAutomatedReasoningPolicyBuildWorkflow` — not a clock transition. The build it
    /// was running is being stopped, so the workflow is owed no completion any more.
    pub fn cancel(&mut self, now: SystemTime) {
        self.status = BUILD_WORKFLOW_STATUS_CANCEL_REQUESTED;
        self.cancelled_due_at = Some(now + BUILD_WORKFLOW_CANCELLED_AFTER);
        self.completed_due_at = None;
        self.updated_at = now;
    }
}

/// A stored Automated Reasoning policy: the identity `CreateAutomatedReasoningPolicy`
/// minted, the draft definition every later read of that policy reports, and every
/// numbered version snapshotted from that draft.
///
/// The `definitionHash` is held beside the definition rather than recomputed per read, so
/// the hash the create answered is the hash the read reports; it is a function of the
/// DEFINITION alone ([`policy_definition_hash`]), which is what makes it usable as the
/// concurrency token its member documents.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyRecord {
    /// The `AutomatedReasoningPolicyArn` this policy is addressed by, minted by
    /// [`automated_reasoning_policy_arn`] — also the key this family's store is keyed by.
    pub arn: String,
    /// `policyId` — the resource half of [`Self::arn`] (`^[a-z0-9]{12}$`), which
    /// `GetAutomatedReasoningPolicy` reports beside it.
    pub id: String,
    /// `name`, as the create request spelled it.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN, exactly as
    /// [`CustomModelRecord::region`] is.
    pub region: String,
    /// `description`, as the create request carried it (absent when it carried none).
    pub description: Option<String>,
    /// `policyDefinition`, as the create request carried it (absent when it carried none,
    /// which is how a policy whose rules a build workflow is to extract starts out).
    pub definition: Option<AutomatedReasoningPolicyDefinition>,
    /// `definitionHash` — [`policy_definition_hash`] of [`Self::definition`].
    pub definition_hash: String,
    /// `kmsKeyArn` — the customer managed key the create named, completed to a `KmsKeyArn`
    /// by [`kms_key_arn`], or `None` when the create named none (Amazon Bedrock then uses
    /// an AWS managed key, which is no key of this account to report).
    pub kms_key_arn: Option<String>,
    /// `tags` — the tags the create request attached to the policy.
    ///
    /// Kept for the reason [`CustomModelRecord::tags`] is: it is part of the request that
    /// minted the record, and the policy's own reads report no tag member.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// When the policy was last written. The creation instant until an update rewrites
    /// the draft.
    pub updated_at: SystemTime,
    /// Every numbered version cut from this policy's draft, keyed by its version number —
    /// the `:<version>` suffix an `AutomatedReasoningPolicyArn` carries.
    ///
    /// Held INSIDE the policy record, like a guardrail's [`Guardrail::versions`], so a
    /// version can neither outlive the policy it was cut from nor be reached under another
    /// one. A version is a SNAPSHOT: nothing rewrites an entry once it is inserted, which
    /// is what makes a later write to the draft invisible to it.
    pub versions: BTreeMap<u32, AutomatedReasoningPolicyVersionRecord>,
    /// `CreateAutomatedReasoningPolicyVersion` idempotency: `clientRequestToken` -> the
    /// version number that token cut.
    ///
    /// Inside the record for the reason [`Guardrail::version_tokens`] is: it holds at most
    /// one entry per version of this policy, and dies with the policy by construction.
    pub version_tokens: HashMap<String, u32>,
    /// The number `CreateAutomatedReasoningPolicyVersion` will mint next, from `1` upwards
    /// — the smallest number `AutomatedReasoningPolicyVersion` (`^([1-9][0-9]{0,11})$`)
    /// admits.
    pub next_version: u32,
}

impl AutomatedReasoningPolicyRecord {
    /// The version `number` names, or `None` when this policy has cut no such version.
    #[must_use]
    pub fn version(&self, number: u32) -> Option<&AutomatedReasoningPolicyVersionRecord> {
        self.versions.get(&number)
    }
}

/// The most numbered versions one Automated Reasoning policy may hold at once, past which
/// cutting another is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota: version numbers are never
/// reused and no operation of this service removes a single version, so without a
/// server-side cap a caller could grow one policy's snapshots without end.
pub const MAX_AUTOMATED_REASONING_POLICY_VERSIONS_PER_POLICY: usize = 100;

/// One numbered version of an Automated Reasoning policy: the snapshot
/// `CreateAutomatedReasoningPolicyVersion` cut from the policy's draft, which every later
/// read of that version reports.
///
/// The definition and its hash are COPIED out of the draft at the instant the version was
/// cut rather than borrowed from it, which is what makes the version immutable: a later
/// write to the draft changes the draft's hash and leaves every version's alone.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyVersionRecord {
    /// The versioned `AutomatedReasoningPolicyArn` this version is addressed by, minted by
    /// [`automated_reasoning_policy_version_arn`].
    pub arn: String,
    /// `version` — the number this snapshot was cut as, from `1` upwards.
    pub version: u32,
    /// `name`, as the policy carried it when the version was cut.
    pub name: String,
    /// `description`, as the policy carried it when the version was cut (absent when it
    /// carried none).
    pub description: Option<String>,
    /// The policy definition this version froze, absent when the draft carried none.
    pub definition: Option<AutomatedReasoningPolicyDefinition>,
    /// `definitionHash` — [`policy_definition_hash`] of [`Self::definition`], so the hash
    /// the version answers is the hash of what it froze.
    pub definition_hash: String,
    /// `tags` — the tags the create-version request attached to this version.
    pub tags: Vec<Tag>,
    /// `createdAt` — when this version was cut. Also the instant a read reports as its
    /// last update, since nothing rewrites a version.
    pub created_at: SystemTime,
}

/// The most tests one Automated Reasoning policy may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// `AutomatedReasoningPolicyTestCaseList` is `@length(0, 1000)`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicyTestCases")`), so
/// a policy holding more tests than this could not be listed; the bound is also what keeps
/// this part of the store from growing without end, since a test carries no name a second
/// create of it could collide with.
pub const MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY: usize = 1000;

/// What a request addressing a test this store does not hold answers as its
/// `ResourceNotFoundException` message, wherever that request came from: a read of the test
/// itself, or a read of a run of it.
///
/// RECORDED behaviour: Amazon Bedrock names the test and then the policy the request
/// addressed, so the wording is fixed and both ids are variable parts taken from the
/// request. Stated once here because the message belongs to the RESOURCE rather than to one
/// operation, so every operation that addresses a test refuses it the same way.
#[must_use]
pub fn automated_reasoning_policy_test_case_not_found_message(
    test_case_id: &str,
    policy_arn: &str,
) -> String {
    format!(
        "The TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn} does not exist"
    )
}

/// What a write to a test whose `lastUpdatedAt` concurrency token is not the one the stored
/// test holds answers as its `ConflictException` message, for the write named by `action`
/// ("deleting", "updating").
///
/// RECORDED behaviour: Amazon Bedrock states the conflict, the write it conflicts with, and
/// then the same two ids
/// [`automated_reasoning_policy_test_case_not_found_message`] carries — so the wording is
/// fixed, its variable parts are the ids the REQUEST carried, and no instant appears in it:
/// neither the one the test holds nor the one the request submitted. Stated once here
/// because every write of a test is held against that token the same way.
#[must_use]
pub fn automated_reasoning_policy_test_case_conflict_message(
    action: &str,
    test_case_id: &str,
    policy_arn: &str,
) -> String {
    format!("Conflict with {action} TestCase with testCaseId: {test_case_id} and policyArn: {policy_arn}")
}

/// The `ConflictException` message a request whose `lastUpdatedDefinitionHash` is not the
/// DRAFT policy's current definition hash is refused with.
///
/// RECORDED behaviour: Amazon Bedrock states that the hash the request PROVIDED is not the
/// draft's — a wholly fixed sentence, carrying neither the policy ARN nor either hash — so
/// an optimistic-concurrency client matches it to decide "re-read the policy and retry".
pub const AUTOMATED_REASONING_POLICY_DEFINITION_HASH_CONFLICT: &str =
    "Provided definition hash does not match the DRAFT policy definition hash.";

/// The `ResourceInUseException` message a request to start a test workflow for a build
/// workflow whose tests are still running is refused with.
///
/// RECORDED behaviour: a wholly fixed sentence — no build workflow id and no status name, so
/// the refusal couples a caller to nothing but the fact that testing is under way, which is
/// what it backs off and retries on.
pub const AUTOMATED_REASONING_POLICY_TEST_WORKFLOW_IN_PROGRESS: &str =
    "Testing workflow is currently in progress";

/// What a request whose `AutomatedReasoningPolicyArn` carries a `:<version>` suffix the
/// addressed policy has not cut answers as its `ResourceNotFoundException` message.
///
/// RECORDED behaviour: Amazon Bedrock reports the missing VERSION — a wholly fixed sentence,
/// carrying neither the ARN nor the number — rather than reporting the policy as absent, so
/// a caller learns the policy is there and the revision it addressed is not.
pub const AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND: &str = "Policy version does not exist";

/// What a request addressing an Automated Reasoning policy this account holds no record of
/// answers as its `ResourceNotFoundException` message, wherever that request came from: a
/// read of the policy, a write of it, a listing of its children, or a call addressing one of
/// those children.
///
/// RECORDED behaviour, and a message with NO variable parts: Amazon Bedrock reports that the
/// policy is not there and names neither the ARN the request carried nor the account it
/// looked in, so the wording is the whole requirement and is shared by every operation that
/// resolves an `AutomatedReasoningPolicyArn` — the message belongs to the RESOURCE, not to
/// one operation. Worded as its sibling [`AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND`] is,
/// which reports the missing VERSION of a policy that IS there.
pub const AUTOMATED_REASONING_POLICY_NOT_FOUND: &str = "Policy does not exist";

/// One stored Automated Reasoning policy test: the identity
/// `CreateAutomatedReasoningPolicyTestCase` minted, and the sample input and expected
/// outcome every later read of that test reports.
///
/// Every member is held exactly as the request carried it, because the test is what a
/// policy is validated against: `guardContent` and `queryContent` are opaque text whose
/// characters are the test, so they are stored byte for byte rather than normalised.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyTestCaseRecord {
    /// `testCaseId` — the `^[0-9A-Z]{12}$` identity minted by
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::mint_automated_reasoning_policy_test_case_id`].
    pub id: String,
    /// `guardContent` — the output content the policy validates, as the request spelled it.
    pub guard_content: String,
    /// `queryContent` — the query that produced that content, absent when the request
    /// carried none.
    pub query_content: Option<String>,
    /// `expectedAggregatedFindingsResult` — the `AutomatedReasoningCheckResult` the test
    /// expects. Required on the create, so a stored test always carries one.
    pub expected_aggregated_findings_result: String,
    /// `confidenceThreshold` — the minimum translation confidence this test validates at,
    /// absent when the request carried none.
    pub confidence_threshold: Option<f64>,
    pub created_at: SystemTime,
    /// When the test was last written. The creation instant until an update rewrites it.
    pub updated_at: SystemTime,
    /// The `clientRequestToken` the write this test currently holds was made under, when
    /// that write was an UPDATE that carried one, and `None` while the test still holds
    /// what the create wrote.
    ///
    /// `UpdateAutomatedReasoningPolicyTestCase$clientRequestToken` is an
    /// `@idempotencyToken` whose purpose is that "the operation completes no more than one
    /// time", so the token that produced the current write is kept WITH that write: a
    /// retried update carries it beside the `lastUpdatedAt` the retried request was built
    /// from, which the landed write has already moved past. Holding it here — rather than
    /// in a map beside the store — is what lets the retry be recognised and ignored, and
    /// makes the token go with the test when the test is deleted.
    pub update_client_request_token: Option<String>,
}

/// `AutomatedReasoningPolicyTestRunStatus$SCHEDULED` — the status a test run that a test
/// workflow has just accepted carries, before the clock owes it its completion
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyTestRunStatus")`).
pub const TEST_RUN_STATUS_SCHEDULED: &str = "SCHEDULED";

/// `AutomatedReasoningPolicyTestRunStatus$COMPLETED` — the terminal status the clock owes a
/// scheduled test run [`TEST_RUN_COMPLETE_AFTER`] from the workflow that scheduled it.
pub const TEST_RUN_STATUS_COMPLETED: &str = "COMPLETED";

/// `AutomatedReasoningPolicyTestRunStatus$FAILED` — the other terminal status the enum
/// admits. Nothing in this emulator moves a run to it, and it is named here so
/// [`AutomatedReasoningPolicyTestRunRecord::is_active`] decides on the whole set of
/// terminal statuses rather than on the one this emulator produces.
pub const TEST_RUN_STATUS_FAILED: &str = "FAILED";

/// How long after `StartAutomatedReasoningPolicyTestWorkflow` accepts a test run its
/// `testRunStatus` is owed its move to [`TEST_RUN_STATUS_COMPLETED`].
///
/// The emulator's own span, chosen like [`BUILD_WORKFLOW_COMPLETE_AFTER`] rather than
/// documented by AWS: a run the workflow answers in a non-terminal status is owed a
/// transition, and the span is non-zero so a caller reading straight after the start still
/// sees the status that start recorded.
pub const TEST_RUN_COMPLETE_AFTER: Duration = Duration::from_millis(500);

/// One run of one stored test against one build workflow of its policy: what
/// `StartAutomatedReasoningPolicyTestWorkflow` schedules and
/// `GetAutomatedReasoningPolicyTestResult` reports.
///
/// A run is keyed by the TRIPLE `(policyArn, buildWorkflowId, testCaseId)` its read is
/// addressed by (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyTestResult")`),
/// so running the same test against two build workflows of a policy is two runs and
/// re-running it against one build workflow replaces that build workflow's run of it.
///
/// The test's own content is not copied here: a result reports the stored test
/// ([`AutomatedReasoningPolicyTestCaseRecord`]) beside the run, so there is only ever one
/// copy of a test whichever operation reads it.
///
/// `status` is a `&'static str` for the reason
/// [`AutomatedReasoningPolicyBuildWorkflowRecord::status`] is: it is a value this emulator
/// derives rather than one a request names.
#[derive(Debug, Clone)]
pub struct AutomatedReasoningPolicyTestRunRecord {
    /// The `AutomatedReasoningPolicyArn` of the policy whose test this run ran — the first
    /// third of the key this family's store is keyed by.
    pub policy_arn: String,
    /// The `buildWorkflowId` of the build workflow this run tested — the second third of
    /// that key.
    pub build_workflow_id: String,
    /// The `testCaseId` of the test this run ran — the last third of that key.
    pub test_case_id: String,
    /// `testRunStatus` (`AutomatedReasoningPolicyTestRunStatus`).
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`TEST_RUN_STATUS_COMPLETED`], or `None`
    /// once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`AutomatedReasoningPolicyBuildWorkflowRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// `updatedAt` — when this run last changed: the instant the workflow scheduled it
    /// until the clock moves its status.
    pub updated_at: SystemTime,
}

/// The test workflow a build workflow last accepted: the `clientRequestToken` that request
/// carried and the `testCaseIds` it selected (`None` when it named none, which selects
/// every test of the policy).
///
/// What `StartAutomatedReasoningPolicyTestWorkflow` recognises a request it has already
/// accepted by, which is why the selection is held beside the token: a request naming other
/// tests asks for other work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedAutomatedReasoningPolicyTestWorkflow {
    /// `clientRequestToken`, as the accepted request carried it.
    pub client_request_token: String,
    /// `testCaseIds`, as that request carried it.
    pub test_case_ids: Option<Vec<String>>,
}

impl AutomatedReasoningPolicyTestRunRecord {
    /// One run of `test_case_id` against `build_workflow_id`, as the test workflow that
    /// accepted it at `now` leaves it: the ONE place a run's initial state is written, so a
    /// fresh run and a re-run start alike (both go on through [`Self::schedule`]).
    #[must_use]
    pub fn scheduled(
        policy_arn: &str,
        build_workflow_id: &str,
        test_case_id: &str,
        now: SystemTime,
    ) -> Self {
        let mut record = Self {
            policy_arn: policy_arn.to_string(),
            build_workflow_id: build_workflow_id.to_string(),
            test_case_id: test_case_id.to_string(),
            status: TEST_RUN_STATUS_SCHEDULED,
            completed_due_at: None,
            updated_at: now,
        };
        record.schedule(now);
        record
    }

    /// Whether this run is still running, so the build workflow it is testing is in use by
    /// it: only a terminal status frees that build workflow for another test workflow.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(
            self.status,
            TEST_RUN_STATUS_COMPLETED | TEST_RUN_STATUS_FAILED
        )
    }

    /// Materialise the status transition the clock has already made due at `now`: the
    /// completion the workflow that scheduled this run owes it.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the run reports as its last update.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = TEST_RUN_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.updated_at = due;
        }
    }

    /// Schedule this run as of `now`: the write
    /// `StartAutomatedReasoningPolicyTestWorkflow` makes, for a run it has just accepted
    /// and for one it is re-running.
    pub fn schedule(&mut self, now: SystemTime) {
        self.status = TEST_RUN_STATUS_SCHEDULED;
        self.completed_due_at = Some(now + TEST_RUN_COMPLETE_AFTER);
        self.updated_at = now;
    }
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

/// What this control plane answers when a request addresses an enforced guardrail
/// configuration this account does not hold: the `configId` names no configuration, which
/// is the 404 `ResourceNotFoundException` both the put that addresses one and the delete
/// that removes one answer. Stated once here because two operations make the same refusal
/// for the same reason and two spellings of it could drift.
#[must_use]
pub fn enforced_guardrail_configuration_not_found(config_id: &str) -> String {
    format!("no enforced guardrail configuration {config_id} exists in this account")
}

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

    /// Forget the prompt router `arn` names, together with its name index entry and the
    /// idempotency token that minted it — so a delete leaves nothing behind for a later
    /// create of the same name to collide with or for a replayed token to hand back, and
    /// both maps stay bounded by the LIVE routers exactly as [`Self::remove_custom_model`]
    /// keeps that family's maps bounded.
    ///
    /// Hands back the record that was removed, or `None` when this store held no router
    /// under that ARN — which is what tells a delete there was nothing to remove.
    pub fn remove_prompt_router(&mut self, arn: &str) -> Option<PromptRouter> {
        let router = self.prompt_routers.remove(arn)?;
        self.prompt_router_names.retain(|_, minted| minted != arn);
        self.prompt_router_create_tokens
            .retain(|_, minted| minted != arn);
        Some(router)
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

    /// Mint a fresh Automated Reasoning policy id: the resource half of an
    /// `automated-reasoning-policy/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `AutomatedReasoningPolicyArn` admits exactly `[a-z0-9]{12}` there, which is what
    /// two [`ID_HALF_WIDTH`] base-36 halves spell, and `AutomatedReasoningPolicyId` is
    /// that same `^[a-z0-9]{12}$`.
    pub fn mint_automated_reasoning_policy_id(&mut self, now: SystemTime) -> String {
        self.minted_automated_reasoning_policies += 1;
        minted_id(self.minted_automated_reasoning_policies, now)
    }

    /// The Automated Reasoning policy an `AutomatedReasoningPolicyArn` names.
    ///
    /// The lookup is EXACT, so the ARN's region and its optional `:<version>` suffix both
    /// select: only the base ARN names the policy whose draft this store holds. Reach a
    /// version through [`Self::automated_reasoning_policy_revision`], which resolves either
    /// spelling.
    #[must_use]
    pub fn automated_reasoning_policy(
        &self,
        policy_arn: &str,
    ) -> Option<&AutomatedReasoningPolicyRecord> {
        self.automated_reasoning_policies.get(policy_arn)
    }

    /// The Automated Reasoning policy an `AutomatedReasoningPolicyArn` names, mutably.
    pub fn automated_reasoning_policy_mut(
        &mut self,
        policy_arn: &str,
    ) -> Option<&mut AutomatedReasoningPolicyRecord> {
        self.automated_reasoning_policies.get_mut(policy_arn)
    }

    /// The REVISION an `AutomatedReasoningPolicyArn` names: the policy, and the version its
    /// optional `:<version>` suffix selects.
    ///
    /// The suffix is what the shape's own pattern admits beside the base ARN
    /// (`^arn:…:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$`), so a
    /// versioned ARN names a version this policy has cut and an unversioned one names its
    /// working draft. A version the policy has not cut names no revision at all, exactly as
    /// an id nothing minted names no policy.
    #[must_use]
    pub fn automated_reasoning_policy_revision(
        &self,
        policy_arn: &str,
    ) -> Option<(
        &AutomatedReasoningPolicyRecord,
        Option<&AutomatedReasoningPolicyVersionRecord>,
    )> {
        let (base_arn, version) = automated_reasoning_policy_arn_parts(policy_arn);
        let policy = self.automated_reasoning_policies.get(base_arn)?;
        match version {
            None => Some((policy, None)),
            Some(number) => Some((policy, Some(policy.version(number)?))),
        }
    }

    /// The `ResourceNotFoundException` message an `AutomatedReasoningPolicyArn` that
    /// [`Self::automated_reasoning_policy_revision`] resolves to NOTHING is refused with.
    ///
    /// WHICH half of the ARN resolved to nothing decides the sentence: an ARN naming a
    /// version of a policy this account DOES hold addressed a revision that policy has not
    /// cut, so the refusal reports the missing version
    /// ([`AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND`]) rather than the policy, which is
    /// there. Here rather than in one handler because every operation addressed by this ARN
    /// reports one unresolvable ARN the same way, and that wording is one rule.
    #[must_use]
    pub fn automated_reasoning_policy_revision_not_found_message(&self, policy_arn: &str) -> String {
        let (base_arn, version) = automated_reasoning_policy_arn_parts(policy_arn);
        if version.is_some() && self.automated_reasoning_policy(base_arn).is_some() {
            AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND.to_string()
        } else {
            AUTOMATED_REASONING_POLICY_NOT_FOUND.to_string()
        }
    }

    /// Mint a fresh `AutomatedReasoningPolicyBuildWorkflowId`: the lowercase hyphenated
    /// UUIDv4 that member's pattern spells
    /// (`^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`),
    /// unique within this store because [`minted_uuid`] hashes this store's mint count in.
    pub fn mint_build_workflow_id(&mut self, now: SystemTime) -> String {
        self.minted_automated_reasoning_policy_build_workflows += 1;
        minted_uuid(
            self.minted_automated_reasoning_policy_build_workflows,
            now,
        )
    }

    /// The build workflow `(policy_arn, build_workflow_id)` names, with any status
    /// transition the clock has made due at `now` materialised first — so every caller
    /// reads the status the workflow holds at `now`, whichever operation asked.
    pub fn automated_reasoning_policy_build_workflow(
        &mut self,
        policy_arn: &str,
        build_workflow_id: &str,
        now: SystemTime,
    ) -> Option<&mut AutomatedReasoningPolicyBuildWorkflowRecord> {
        let record = self
            .automated_reasoning_policy_build_workflows
            .get_mut(&(policy_arn.to_string(), build_workflow_id.to_string()))?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every build workflow transition of the policy `policy_arn` names that
    /// the clock has made due at `now`, so a caller asking about the POLICY sees the same
    /// workflows a read of each one would.
    pub fn settle_automated_reasoning_policy_build_workflows(
        &mut self,
        policy_arn: &str,
        now: SystemTime,
    ) {
        for ((arn, _), record) in &mut self.automated_reasoning_policy_build_workflows {
            if arn == policy_arn {
                record.settle(now);
            }
        }
    }

    /// Every build workflow started for the policy `policy_arn` names, as this store holds
    /// them now (settle first, with
    /// [`Self::settle_automated_reasoning_policy_build_workflows`], to read them as of an
    /// instant).
    pub fn automated_reasoning_policy_build_workflows_of<'a>(
        &'a self,
        policy_arn: &'a str,
    ) -> impl Iterator<Item = &'a AutomatedReasoningPolicyBuildWorkflowRecord> + 'a {
        self.automated_reasoning_policy_build_workflows
            .iter()
            .filter(move |((arn, _), _)| arn == policy_arn)
            .map(|(_, record)| record)
    }

    /// Mint a fresh Automated Reasoning policy test id, unique within this store.
    ///
    /// Built like a policy id, and for the same two reasons: the leading half is this
    /// store's mint count so two ids can never collide, and the trailing half is drawn
    /// from the creation instant so an id is not a guessable sequence number. Rendered in
    /// UPPER case because `AutomatedReasoningPolicyTestCaseId` is `^[0-9A-Z]{12}$`, which
    /// is the upper-case spelling of the twelve base-36 digits [`minted_id`] produces.
    pub fn mint_automated_reasoning_policy_test_case_id(&mut self, now: SystemTime) -> String {
        self.minted_automated_reasoning_policy_test_cases += 1;
        minted_id(self.minted_automated_reasoning_policy_test_cases, now).to_ascii_uppercase()
    }

    /// The test `test_case_id` names under the policy `policy_arn` names.
    ///
    /// Keyed by the PAIR, so an id of another policy's test names no test of this one, and
    /// the policy ARN selects exactly as [`Self::automated_reasoning_policy`] does.
    #[must_use]
    pub fn automated_reasoning_policy_test_case(
        &self,
        policy_arn: &str,
        test_case_id: &str,
    ) -> Option<&AutomatedReasoningPolicyTestCaseRecord> {
        self.automated_reasoning_policy_test_cases
            .get(&(policy_arn.to_string(), test_case_id.to_string()))
    }

    /// Every test stored under the policy `policy_arn` names, in `testCaseId` order.
    ///
    /// Keyed by the PAIR like [`Self::automated_reasoning_policy_test_case`], so a listing
    /// reports exactly the tests a read of each one reports and none of another policy's.
    /// The order is the id's, which is a TOTAL order over
    /// `AutomatedReasoningPolicyTestCaseId` (`^[0-9A-Z]{12}$`): a listing needs one so its
    /// pages partition the tests whatever is created or deleted between two calls.
    #[must_use]
    pub fn automated_reasoning_policy_test_cases_of(
        &self,
        policy_arn: &str,
    ) -> Vec<&AutomatedReasoningPolicyTestCaseRecord> {
        let mut records: Vec<&AutomatedReasoningPolicyTestCaseRecord> = self
            .automated_reasoning_policy_test_cases
            .iter()
            .filter(|((arn, _), _)| arn == policy_arn)
            .map(|(_, record)| record)
            .collect();
        records.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        records
    }

    /// How many tests the policy `policy_arn` names holds — what
    /// [`MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY`] bounds.
    #[must_use]
    pub fn automated_reasoning_policy_test_case_count(&self, policy_arn: &str) -> usize {
        self.automated_reasoning_policy_test_cases
            .keys()
            .filter(|(arn, _)| arn == policy_arn)
            .count()
    }

    /// The run of the test `test_case_id` against the build workflow `build_workflow_id` of
    /// the policy `policy_arn` names, with any status transition the clock has made due at
    /// `now` materialised first — so every caller reads the status the run holds at `now`,
    /// whichever operation asked.
    pub fn automated_reasoning_policy_test_run(
        &mut self,
        policy_arn: &str,
        build_workflow_id: &str,
        test_case_id: &str,
        now: SystemTime,
    ) -> Option<&mut AutomatedReasoningPolicyTestRunRecord> {
        let record = self.automated_reasoning_policy_test_runs.get_mut(&(
            policy_arn.to_string(),
            build_workflow_id.to_string(),
            test_case_id.to_string(),
        ))?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every test run transition of the build workflow `build_workflow_id` of
    /// the policy `policy_arn` names that the clock has made due at `now`, so a caller
    /// asking about the BUILD WORKFLOW sees the same runs a read of each one would.
    pub fn settle_automated_reasoning_policy_test_runs(
        &mut self,
        policy_arn: &str,
        build_workflow_id: &str,
        now: SystemTime,
    ) {
        for ((arn, workflow, _), record) in &mut self.automated_reasoning_policy_test_runs {
            if arn == policy_arn && workflow == build_workflow_id {
                record.settle(now);
            }
        }
    }

    /// Every run scheduled against the build workflow `build_workflow_id` of the policy
    /// `policy_arn` names, in `testCaseId` order (settle first, with
    /// [`Self::settle_automated_reasoning_policy_test_runs`], to read them as of an
    /// instant).
    ///
    /// The order is the test id's, which is a TOTAL order over
    /// `AutomatedReasoningPolicyTestCaseId` (`^[0-9A-Z]{12}$`), exactly as the tests
    /// themselves are ordered.
    #[must_use]
    pub fn automated_reasoning_policy_test_runs_of(
        &self,
        policy_arn: &str,
        build_workflow_id: &str,
    ) -> Vec<&AutomatedReasoningPolicyTestRunRecord> {
        let mut records: Vec<&AutomatedReasoningPolicyTestRunRecord> = self
            .automated_reasoning_policy_test_runs
            .iter()
            .filter(|((arn, workflow, _), _)| arn == policy_arn && workflow == build_workflow_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_unstable_by(|left, right| left.test_case_id.cmp(&right.test_case_id));
        records
    }

    /// Mint a fresh inference profile id: the resource half of an
    /// `application-inference-profile/<id>` ARN, unique within this store.
    ///
    /// Built like a prompt router id, and for the same two reasons: the leading half is
    /// this store's mint count so two ids can never collide, and the trailing half is
    /// drawn from the creation instant so an id is not a guessable sequence number.
    /// `[a-z0-9]+` is inside the `[a-zA-Z0-9-:.]+` that `InferenceProfileArn` admits
    /// there, and inside the `^[a-zA-Z0-9-:.]+$` of `InferenceProfileId`.
    pub fn mint_inference_profile_id(&mut self, now: SystemTime) -> String {
        self.minted_inference_profiles += 1;
        minted_id(self.minted_inference_profiles, now)
    }

    /// The `InferenceProfileArn` an `InferenceProfileIdentifier` names: the identifier
    /// itself when it is the ARN of a stored profile, otherwise the ARN the profile of
    /// that `inferenceProfileId` carries in `region`.
    ///
    /// The identifier's pattern admits that ARN or a bare profile id
    /// (`smithy_query(query="operation",operation="GetInferenceProfile")`), so both
    /// spellings address the same profile. Either spelling resolves only to a profile of
    /// `region`: the ARN carries a region, so one of another region names another
    /// account-region's profile.
    #[must_use]
    pub fn inference_profile_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if let Some(record) = self.inference_profiles.get(identifier) {
            return (record.region == region).then(|| identifier.to_string());
        }
        let spelled = application_inference_profile_arn(region, identifier);
        self.inference_profiles
            .contains_key(&spelled)
            .then_some(spelled)
    }

    /// The application inference profile an `InferenceProfileIdentifier` names.
    #[must_use]
    pub fn inference_profile(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<&InferenceProfileRecord> {
        let arn = self.inference_profile_arn_of(identifier, region)?;
        self.inference_profiles.get(&arn)
    }

    /// Forget the application inference profile `arn` names, together with the idempotency
    /// tokens that minted it — so the token map stays bounded by the LIVE profiles rather
    /// than by every create this store has ever admitted.
    pub fn remove_inference_profile(&mut self, arn: &str) -> Option<InferenceProfileRecord> {
        let record = self.inference_profiles.remove(arn)?;
        self.inference_profile_create_tokens
            .retain(|_, minted| minted != arn);
        Some(record)
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

    /// Drop the marketplace model endpoint an `endpointArn` names, handing back the record
    /// that was removed (`None` when this store held none under that ARN).
    ///
    /// The create tokens that minted it go with it, the way
    /// [`Self::remove_inference_profile`] drops the tokens that minted a profile: the token
    /// map stays bounded by the LIVE endpoints rather than by every create this store has
    /// ever admitted, and a token whose endpoint is gone cannot replay an endpoint back
    /// into being.
    pub fn remove_marketplace_model_endpoint(
        &mut self,
        arn: &str,
    ) -> Option<MarketplaceModelEndpointRecord> {
        let endpoint = self.marketplace_model_endpoints.remove(arn)?;
        self.marketplace_endpoint_create_tokens
            .retain(|_, minted| minted != arn);
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
    ///
    /// A job a batch delete accepted and whose removal the clock has since made due is gone
    /// by the time it is asked for, so an identifier that named it names nothing — the one
    /// other transition this resource is owed, and the reason the removal is swept here
    /// rather than inside [`EvaluationJobRecord::settle`], which cannot reach the store the
    /// record is held in.
    pub fn evaluation_job(
        &mut self,
        identifier: &str,
        now: SystemTime,
    ) -> Option<&mut EvaluationJobRecord> {
        self.expire_deleted_evaluation_jobs(now);
        let job = self.evaluation_jobs.get_mut(identifier)?;
        job.settle(now);
        Some(job)
    }

    /// Forget every evaluation job whose removal the clock has already made due at `now`.
    ///
    /// Swept over the whole family rather than over the one identifier asked for, because a
    /// removal frees the job's NAME and its place under the account's bound: both are read
    /// without an identifier in hand.
    pub fn expire_deleted_evaluation_jobs(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .evaluation_jobs
            .iter()
            .filter(|(_, job)| job.is_removal_due(now))
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in due {
            self.remove_evaluation_job(&arn);
        }
    }

    /// Forget the evaluation job `arn` names, together with its name index entry and the
    /// idempotency tokens that minted it — so a deletion leaves nothing behind for a later
    /// create of the same name to collide with, exactly as
    /// [`Self::remove_provisioned_model_throughput`] does for that family.
    pub fn remove_evaluation_job(&mut self, arn: &str) -> Option<EvaluationJobRecord> {
        let job = self.evaluation_jobs.remove(arn)?;
        self.evaluation_job_names
            .remove(&(job.region.clone(), job.name.clone()));
        self.evaluation_job_create_tokens
            .retain(|_, minted| minted != arn);
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

    /// Mint a fresh advanced prompt optimization job id: the resource half of an
    /// `advanced-prompt-optimization-job/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `AdvancedPromptOptimizationJobArn` admits exactly `[a-z0-9]{12}` there, which is
    /// what two [`ID_HALF_WIDTH`] base-36 halves spell, and
    /// `AdvancedPromptOptimizationJobIdentifier`'s bare alternative is that same
    /// `[a-z0-9]{12}`.
    pub fn mint_advanced_prompt_optimization_job_id(&mut self, now: SystemTime) -> String {
        self.minted_advanced_prompt_optimization_jobs += 1;
        minted_id(self.minted_advanced_prompt_optimization_jobs, now)
    }

    /// The `AdvancedPromptOptimizationJobArn` an `AdvancedPromptOptimizationJobIdentifier`
    /// names: the identifier itself when it is the ARN of a stored job, otherwise the ARN
    /// the job of that ID carries in `region`.
    ///
    /// The identifier's pattern is that ARN with its `advanced-prompt-optimization-job/<id>`
    /// tail, or the bare `<id>` alone
    /// (`smithy_query(query="operation",operation="GetAdvancedPromptOptimizationJob")`), so
    /// both spellings address the same job — resolved exactly as
    /// [`Self::model_invocation_job_arn_of`] resolves a batch inference job's two
    /// spellings.
    #[must_use]
    pub fn advanced_prompt_optimization_job_arn_of(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<String> {
        if self
            .advanced_prompt_optimization_jobs
            .contains_key(identifier)
        {
            return Some(identifier.to_string());
        }
        let arn = advanced_prompt_optimization_job_arn(region, identifier);
        self.advanced_prompt_optimization_jobs
            .contains_key(&arn)
            .then_some(arn)
    }

    /// The advanced prompt optimization job an `AdvancedPromptOptimizationJobIdentifier`
    /// names, with any status transition the clock has made due at `now` materialised first
    /// — so every caller reads the status the job holds at `now`, whichever operation asked.
    pub fn advanced_prompt_optimization_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut AdvancedPromptOptimizationJobRecord> {
        let arn = self.advanced_prompt_optimization_job_arn_of(identifier, region)?;
        let record = self.advanced_prompt_optimization_jobs.get_mut(&arn)?;
        record.settle(now);
        Some(record)
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
        // A customization job the clock has already completed is what brings its output
        // model into being, and so is a copy job the clock has already completed, so those
        // completions are materialised before a model is looked
        // up: such a model was owed at its job's due instant even if nobody had read the
        // job.
        self.settle_model_customization_jobs(now);
        self.settle_model_copy_jobs(now);
        let arn = self.custom_model_arn_of(identifier, region)?;
        let record = self.custom_models.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every custom model status transition the clock has made due at `now`,
    /// so a LISTING reports — and filters on — the status each record holds at `now`
    /// exactly as a read of that one model would.
    pub fn settle_custom_models(&mut self, now: SystemTime) {
        // Including the models whose customization or copy job the clock has completed,
        // for the reason [`Self::custom_model`] materialises those first.
        self.settle_model_customization_jobs(now);
        self.settle_model_copy_jobs(now);
        for record in self.custom_models.values_mut() {
            record.settle(now);
        }
    }

    /// Forget the custom model `arn` names, together with its name index entry and the
    /// idempotency tokens that minted it — so a delete leaves nothing behind for a later
    /// create of the same name to collide with, and the token map stays bounded by the LIVE
    /// custom models, exactly as [`Self::remove_provisioned_model_throughput`] keeps that
    /// family's maps bounded.
    pub fn remove_custom_model(&mut self, arn: &str) -> Option<CustomModelRecord> {
        let record = self.custom_models.remove(arn)?;
        self.custom_model_names
            .remove(&(record.region.clone(), record.name.clone()));
        self.custom_model_create_tokens
            .retain(|_, minted| minted != arn);
        Some(record)
    }

    /// The ARN of the custom model deployment serving the custom model `arn` names, or
    /// `None` when no stored deployment serves it.
    ///
    /// A deployment carries "the custom model this deployment serves"
    /// ([`CustomModelDeploymentRecord::model_arn`]), so it holds a live association to that
    /// one model — the association a delete of the model would leave pointing at a resource
    /// that no longer exists. The SMALLEST such ARN is reported, so a refusal names the same
    /// deployment however the map happens to be ordered.
    ///
    /// Only this family is asked: a Provisioned Throughput of this store is associated with
    /// a FOUNDATION model (`ops/create_provisioned_model_throughput.rs` resolves its
    /// `modelId` through the published catalog and stores that model's ARN), so none of them
    /// can hold an association to a custom model.
    #[must_use]
    pub fn custom_model_deployment_serving(&self, arn: &str) -> Option<String> {
        self.custom_model_deployments
            .values()
            .filter(|record| record.model_arn == arn)
            .map(|record| record.arn.clone())
            .min()
    }

    /// Mint a fresh model customization job id: the resource half of a
    /// `model-customization-job/<base model>/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `ModelCustomizationJobArn` admits exactly `[a-z0-9]{12}` there, which is what two
    /// [`ID_HALF_WIDTH`] base-36 halves spell.
    pub fn mint_model_customization_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_customization_jobs += 1;
        minted_id(self.minted_model_customization_jobs, now)
    }

    /// The `ModelCustomizationJobArn` a `ModelCustomizationJobIdentifier` names: the
    /// identifier itself when it is the ARN of a stored job, otherwise the ARN the job of
    /// that NAME in `region` carries.
    ///
    /// The identifier's pattern is that ARN OR a bare `JobName`
    /// (`smithy_query(query="operation",operation="GetModelCustomizationJob")`), so both
    /// spellings address the same job — resolved exactly as
    /// [`Self::model_import_job_arn_of`] resolves an import job's two spellings.
    #[must_use]
    pub fn model_customization_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.model_customization_jobs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.model_customization_job_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The model customization job a `ModelCustomizationJobIdentifier` names, with any
    /// status transition the clock has made due at `now` materialised first — so every
    /// caller reads the status the job holds at `now`, whichever operation asked.
    pub fn model_customization_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ModelCustomizationJobRecord> {
        let arn = self.model_customization_job_arn_of(identifier, region)?;
        self.settle_model_customization_job(&arn, now);
        self.model_customization_jobs.get_mut(&arn)
    }

    /// Materialise the transition the clock has already made due at `now` for the job
    /// `arn` names: the stop a caller asked for, or the completion that brings the custom
    /// model the job was customizing into being.
    ///
    /// The only write a reporting operation on this resource makes, and it was owed at the
    /// due instant even if nobody had called — which is why that instant, not `now`, is the
    /// one the job reports as its end and the one the model it produced reports as its
    /// creation. The stop takes precedence because [`ModelCustomizationJobRecord::stop`]
    /// drops the completion the job was owed: a job that was stopped produces no model.
    fn settle_model_customization_job(&mut self, arn: &str, now: SystemTime) {
        let completed = {
            let Some(job) = self.model_customization_jobs.get_mut(arn) else {
                return;
            };
            if let Some(due) = job.stopped_due_at.filter(|due| now >= *due) {
                job.status = MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED;
                job.stopped_due_at = None;
                job.ended_at = Some(due);
                job.last_modified_at = due;
                return;
            }
            let Some(due) = job.completed_due_at.filter(|due| now >= *due) else {
                return;
            };
            job.status = MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED;
            job.completed_due_at = None;
            job.ended_at = Some(due);
            job.last_modified_at = due;
            CustomModelRecord {
                arn: job.output_model_arn.clone(),
                name: job.output_model_name.clone(),
                region: job.region.clone(),
                base_model_arn: job.base_model_arn.clone(),
                base_model_name: base_model_id_of(&job.base_model_arn).to_string(),
                // This model descends from the foundation model the job customized, which
                // is what `ListCustomModels`' `foundationModelArnEquals` filters on.
                foundation_model_arn: Some(job.base_model_arn.clone()),
                job_arn: Some(job.arn.clone()),
                customization_type: job.customization_type,
                // The artifacts were produced by the customization rather than named by a
                // request, so the model carries neither source.
                model_source_config: None,
                custom_model_data_source: None,
                model_kms_key_arn: job.output_model_kms_key_arn.clone(),
                role_arn: Some(job.role_arn.clone()),
                tags: job.custom_model_tags.clone(),
                // "After the model-customization job completes successfully, your custom
                // model resource will be ready to use", so the model this registers is
                // ready and the clock owes it no further transition.
                status: CUSTOM_MODEL_STATUS_ACTIVE,
                active_due_at: None,
                created_at: due,
            }
        };
        self.custom_model_names.insert(
            (completed.region.clone(), completed.name.clone()),
            completed.arn.clone(),
        );
        self.custom_models.insert(completed.arn.clone(), completed);
    }

    /// Materialise every model customization job transition the clock has made due at
    /// `now`, so a reader of the resources those jobs PRODUCE sees the same jobs a read of
    /// each one would.
    ///
    /// Lives beside the per-job materialisation because a completion brings a custom model
    /// into being: a caller that reads only the model never touches the job, and the model
    /// was owed at the job's due instant even so.
    pub fn settle_model_customization_jobs(&mut self, now: SystemTime) {
        let arns: Vec<String> = self.model_customization_jobs.keys().cloned().collect();
        for arn in arns {
            self.settle_model_customization_job(&arn, now);
        }
    }

    /// Mint a fresh model copy job id: the resource half of a `model-copy-job/<id>` ARN,
    /// unique within this store.
    ///
    /// Built like a model customization job id, and for the same two reasons;
    /// `ModelCopyJobArn` admits exactly `[a-z0-9]{12}` there.
    pub fn mint_model_copy_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_copy_jobs += 1;
        minted_id(self.minted_model_copy_jobs, now)
    }

    /// The model copy job a `ModelCopyJobArn` names, with any status transition the clock
    /// has made due at `now` materialised first — so every caller reads the status the job
    /// holds at `now`, whichever operation asked.
    pub fn model_copy_job(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&ModelCopyJobRecord> {
        self.settle_model_copy_job(arn, now);
        self.model_copy_jobs.get(arn)
    }

    /// Materialise the completion the clock has already made due at `now` for the job
    /// `arn` names: the copy finishing is what brings the copied custom model into being,
    /// so the completion registers that model beside the account's other custom models
    /// rather than holding a second copy of it.
    ///
    /// The only write a reporting operation on this resource makes, and it was owed at the
    /// due instant even if nobody had called — which is why that instant, not `now`, is the
    /// one the model the copy produced reports as its creation.
    fn settle_model_copy_job(&mut self, arn: &str, now: SystemTime) {
        let copied = {
            let Some(job) = self.model_copy_jobs.get_mut(arn) else {
                return;
            };
            let Some(due) = job.completed_due_at.filter(|due| now >= *due) else {
                return;
            };
            job.status = MODEL_COPY_JOB_STATUS_COMPLETED;
            job.completed_due_at = None;
            CustomModelRecord {
                arn: job.target_model_arn.clone(),
                name: job.target_model_name.clone(),
                region: job.region.clone(),
                // The copy is of the SOURCE model, so the copied model descends from
                // whatever that model descended from: its base is the source model's own
                // base, which is the segment both ARNs carry.
                base_model_arn: job.source_model_arn.clone(),
                base_model_name: custom_model_base_segment(&job.source_model_arn)
                    .unwrap_or(CUSTOM_MODEL_IMPORTED_BASE)
                    .to_string(),
                foundation_model_arn: None,
                job_arn: Some(job.arn.clone()),
                customization_type: CUSTOMIZATION_TYPE_IMPORTED,
                // The artifacts were copied from another model rather than named by a
                // request, so the copied model carries neither source.
                model_source_config: None,
                custom_model_data_source: None,
                model_kms_key_arn: job.target_model_kms_key_arn.clone(),
                role_arn: None,
                tags: job.target_model_tags.clone(),
                // The copy has finished, so the model it produced is ready and the clock
                // owes it no further transition.
                status: CUSTOM_MODEL_STATUS_ACTIVE,
                active_due_at: None,
                created_at: due,
            }
        };
        self.custom_model_names.insert(
            (copied.region.clone(), copied.name.clone()),
            copied.arn.clone(),
        );
        self.custom_models.insert(copied.arn.clone(), copied);
    }

    /// Materialise every model copy job completion the clock has made due at `now`, so a
    /// reader of the custom models those copies PRODUCE sees the same jobs a read of each
    /// one would.
    ///
    /// Lives beside the per-job materialisation for the reason
    /// [`Self::settle_model_customization_jobs`] does: a completion brings a custom model
    /// into being, and a caller that reads only the model never touches the job.
    pub fn settle_model_copy_jobs(&mut self, now: SystemTime) {
        let arns: Vec<String> = self.model_copy_jobs.keys().cloned().collect();
        for arn in arns {
            self.settle_model_copy_job(&arn, now);
        }
    }

    /// Mint a fresh batch inference job id: the resource half of a
    /// `model-invocation-job/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `ModelInvocationJobArn`
    /// admits exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36
    /// halves spell, and `ModelInvocationJobIdentifier`'s bare alternative is that same
    /// `[a-z0-9]{12}`.
    pub fn mint_model_invocation_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_invocation_jobs += 1;
        minted_id(self.minted_model_invocation_jobs, now)
    }

    /// The `ModelInvocationJobArn` a `ModelInvocationJobIdentifier` names: the identifier
    /// itself when it is the ARN of a stored job, otherwise the ARN the job of that ID
    /// carries in `region`.
    ///
    /// The identifier's pattern is that ARN with its `model-invocation-job/<id>` tail, or
    /// the bare `<id>` alone
    /// (`smithy_query(query="operation",operation="GetModelInvocationJob")`), so both
    /// spellings address the same job — resolved exactly as
    /// [`Self::model_customization_job_arn_of`] resolves a customization job's two
    /// spellings, except that the bare spelling here is the minted id rather than a name.
    #[must_use]
    pub fn model_invocation_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.model_invocation_jobs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        let arn = model_invocation_job_arn(region, identifier);
        self.model_invocation_jobs.contains_key(&arn).then_some(arn)
    }

    /// The batch inference job a `ModelInvocationJobIdentifier` names, with every status
    /// transition the clock has made due at `now` materialised first — so every caller reads
    /// the status the job holds at `now`, whichever operation asked and however many times
    /// it has asked before ([`ModelInvocationJobRecord::settle`]).
    pub fn model_invocation_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ModelInvocationJobRecord> {
        let arn = self.model_invocation_job_arn_of(identifier, region)?;
        let record = self.model_invocation_jobs.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise the batch inference job transitions the clock has made due at `now`, so
    /// a listing reports — and its `statusEquals` filters on — the status each job holds at
    /// `now`, which is the status a read of that one job reports at the same instant: a
    /// listing observes every job it carries, exactly as a read observes one
    /// ([`ModelInvocationJobRecord::settle`]).
    pub fn settle_model_invocation_jobs(&mut self, now: SystemTime) {
        for record in self.model_invocation_jobs.values_mut() {
            record.settle(now);
        }
    }

    /// The runs the batch inference jobs of `region` still owe, taken ONCE
    /// ([`ModelInvocationJobRecord::due_run`]) and at most
    /// [`model_invocation_run::MAX_RUNS_PER_REQUEST`] of them per call.
    ///
    /// A listing reports the four record counts of every job it carries, and those counts
    /// are the run's own tally — so a listing takes the runs its jobs are owed exactly as a
    /// read of one job takes that job's, and the two cannot report different counts for one
    /// job. Jobs of another region are another account-region's resources and are not
    /// listed, so their runs are not taken here either.
    ///
    /// The number taken per call is BOUNDED because a listing settles every job of the
    /// Region while the work of one run is bounded by its own input: a job whose run this
    /// call does not take still owes it, so a later call takes it — and the iteration is
    /// lazy, so a run that is not taken is not marked as taken either.
    pub fn due_model_invocation_runs(&mut self, region: &str) -> Vec<model_invocation_run::DueRun> {
        self.model_invocation_jobs
            .values_mut()
            .filter(|record| record.region == region)
            .filter_map(ModelInvocationJobRecord::due_run)
            .take(model_invocation_run::MAX_RUNS_PER_REQUEST)
            .collect()
    }

    /// Whether a stored job in `region` is already customizing a model under
    /// `custom_model_name`.
    ///
    /// A job that was stopped is not: it produces no model, so the name it was going to
    /// give one is free again.
    #[must_use]
    pub fn customizes_model_named(&self, region: &str, custom_model_name: &str) -> bool {
        self.model_customization_jobs.values().any(|job| {
            job.region == region && job.output_model_name == custom_model_name && !job.is_stopped()
        })
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
    /// An application inference profile is addressable by the ARN it was minted with,
    /// resolved through [`Self::inference_profile_arn_of`] — the index
    /// `GetInferenceProfile` resolves an identifier through. (`TaggableResourcesArn`
    /// spells its own bare-name alternative `@length(min: 20)`, which no minted profile id
    /// reaches, so the ARN is the one spelling a tagging call can carry for it.)
    ///
    /// Two families are addressable under two ARN spellings, and both spellings name the
    /// same resource: a prompt router is `default-prompt-router/<id>` to `PromptRouterArn`
    /// and `prompt-router/<id>` here, and an imported model is `imported-model/<id>` to
    /// `ImportedModelArn` and `custom-model/imported/<id>` here. The second spelling is
    /// read back to the first — the one the store is keyed by — in the region the ARN
    /// itself carries, so neither spelling reaches another region's resource.
    ///
    /// The families this resolution does not reach (a custom model, an Automated Reasoning
    /// policy, a model invocation job, an async invoke) resolve to `None`, so a tagging
    /// call that names one is answered as naming no taggable resource of this account;
    /// wiring a family in is that family's own work, not a create's.
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
        if resource_arn.contains(&format!(":{CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE}/")) {
            let record = self.custom_model_deployment(resource_arn, region, now)?;
            return Some(TaggedResource::CustomModelDeployment(record.arn.clone()));
        }
        if resource_arn.contains(":application-inference-profile/") {
            let arn = self.inference_profile_arn_of(resource_arn, region)?;
            return Some(TaggedResource::InferenceProfile(arn));
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
            if let Some(record) = self.custom_model_deployment(resource_arn, region, now) {
                return Some(TaggedResource::CustomModelDeployment(record.arn.clone()));
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
            TaggedResource::CustomModelDeployment(arn) => self
                .custom_model_deployments
                .get(arn)
                .map(|record| &record.tags),
            TaggedResource::InferenceProfile(arn) => {
                self.inference_profiles.get(arn).map(|record| &record.tags)
            }
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
            TaggedResource::CustomModelDeployment(arn) => self
                .custom_model_deployments
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::InferenceProfile(arn) => self
                .inference_profiles
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

    /// The resource a resource-policy call's `resourceArn` names, or `None` when this
    /// account holds no such resource.
    ///
    /// A resource-based policy is attached TO a resource, so the value is resolved through
    /// [`Self::tagged_resource`] — the ONE resolution of a caller-supplied resource ARN in
    /// this service, so a policy reaches exactly the resources a tagging call and that
    /// resource's own reads reach, including the second ARN spelling two of the families
    /// are addressable under. `ResourcePolicyResourceArn` is documented as "the ARN of the
    /// Bedrock resource" (`smithy_query(query="operation",operation="PutResourcePolicy")`)
    /// and declares no `@pattern`, so the ARN form is required HERE: a bare resource name,
    /// which a tagging call admits, is no ARN of this service's namespace and names
    /// nothing to attach a policy to.
    pub fn policy_resource(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<TaggedResource> {
        ::service_common::arn::parse_of(resource_arn, ARN_NAMESPACE)?;
        self.tagged_resource(resource_arn, region, now)
    }

    /// The resource policy attached to the resource a resource-policy call's `resourceArn`
    /// names, or `None` when this account holds no such resource or that resource holds no
    /// policy.
    ///
    /// The document is handed back EXACTLY as it was stored — a policy is a document the
    /// caller wrote, not a shape this emulator re-renders.
    pub fn resource_policy(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&str> {
        let resource = self.policy_resource(resource_arn, region, now)?;
        self.resource_policies.get(&resource).map(String::as_str)
    }

    /// Attach `document` to `resource`, REPLACING the policy that resource already holds —
    /// a resource holds at most one resource policy, which is what makes the put
    /// idempotent.
    ///
    /// Every entry whose resource this store no longer holds is dropped first (a resolved
    /// resource's record is present exactly when the tags read off it are), so the map
    /// stays bounded by the live resources however many attach and delete cycles run.
    pub fn attach_resource_policy(&mut self, resource: TaggedResource, document: String) {
        self.forget_policies_of_gone_resources();
        self.resource_policies.insert(resource, document);
    }

    /// Detach the policy `resource` holds, reporting the document that was in force (or
    /// `None` when that resource held none).
    ///
    /// The counterpart of [`Self::attach_resource_policy`], and bounded the same way: the
    /// entries whose resource this store no longer holds are dropped here too, so a detach
    /// cannot leave a document behind for a resource that is gone.
    pub fn detach_resource_policy(&mut self, resource: &TaggedResource) -> Option<String> {
        self.forget_policies_of_gone_resources();
        self.resource_policies.remove(resource)
    }

    /// Drop every attached policy whose resource this store no longer holds.
    ///
    /// A resolved resource's record is present exactly when the tags read off it are, so a
    /// resource whose record is gone can hold no policy: keeping its document would report
    /// a grant on a resource that no longer exists and would let the map grow without
    /// bound across create/delete cycles.
    fn forget_policies_of_gone_resources(&mut self) {
        let gone: Vec<TaggedResource> = self
            .resource_policies
            .keys()
            .filter(|attached| self.tags_of(attached).is_none())
            .cloned()
            .collect();
        for attached in gone {
            self.resource_policies.remove(&attached);
        }
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
    /// The model LINE this model belongs to: the branded family name its provider publishes
    /// the model under ("Amazon Bedrock foundation models" groups each provider's models by
    /// family, and the model catalogue reports that family beside the model), with the
    /// per-model variant left off — the family of a model is what it shares with its
    /// siblings, so two sizes or two dated revisions of one line carry the same family.
    ///
    /// Stored rather than derived: a family is branded data, so it is published for the
    /// model rather than computable from its id.
    pub model_family: &'static str,
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
    /// The Amazon Bedrock FEATURES published for this model, each out of
    /// [`PUBLISHED_FEATURE_VALUES`].
    ///
    /// Every one of them is a feature Amazon Bedrock layers ON a model — caching a prompt
    /// prefix, routing a prompt between models, optimizing inference latency, running the
    /// model over a dataset — and each publishes the models it supports in its own
    /// "Supported models" table in the Amazon Bedrock User Guide. So it is a published
    /// per-model fact and NOT derivable from what the model does: two models with the same
    /// modalities, streaming and inference types differ here when the feature's table lists
    /// one and not the other, and a model no table lists supports none of them — a freshly
    /// offered model is exactly that case.
    pub features_supported: &'static [&'static str],
    /// `FoundationModelLifecycleStatus` — `ACTIVE` for a model that is available, `LEGACY`
    /// for one that is deprecated
    /// (`crate::ops::types::FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES`).
    pub lifecycle_status: &'static str,
    /// `FoundationModelLifecycle$startOfLifeTime` — "Launch time when the model first
    /// becomes available" (`smithy_query(query="shape",shape="FoundationModelLifecycle")`),
    /// as an RFC 3339 instant: the day AWS made the model available on Amazon Bedrock, taken
    /// from that model's launch, at midnight UTC because what is published for it is a day
    /// rather than an instant. Every offered model has one — a model that is offered has
    /// launched — so this is not an `Option`.
    pub start_of_life_time: &'static str,
    /// `FoundationModelLifecycle$legacyTime` and `$endOfLifeTime` — the legacy window AWS
    /// publishes for a deprecated model ([`LegacyWindow`]), carried by a row whose
    /// [`FoundationModel::lifecycle_status`] is [`LIFECYCLE_LEGACY`] and by no other: a
    /// model that is still available has entered no legacy state and is scheduled for no
    /// end of life, and an instant for one is not something to invent.
    pub legacy_window: Option<LegacyWindow>,
}

/// The legacy window AWS publishes for a deprecated base model ("Model lifecycle" in the
/// Amazon Bedrock User Guide): a model enters legacy state on one date and stops being
/// available on another, and a caller is expected to move off it in between.
///
/// The two instants travel together because they are two ends of ONE window: a model that
/// has entered legacy state has an end of life, and one that has not has neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyWindow {
    /// `FoundationModelLifecycle$legacyTime` — "Time when the model enters legacy state"
    /// (`smithy_query(query="shape",shape="FoundationModelLifecycle")`), as RFC 3339.
    pub entered_at: &'static str,
    /// `FoundationModelLifecycle$endOfLifeTime` — "Time when the model is no longer
    /// available for use", the far end of the window, as RFC 3339.
    pub end_of_life_at: &'static str,
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

/// Every Amazon Bedrock feature a catalog row can publish support for
/// ([`FoundationModel::features_supported`]), in the order a summary reports them.
///
/// Each one has its own "Supported models" table in the Amazon Bedrock User Guide, which is
/// what makes support for it a published per-model fact; the order here is that of the
/// feature names, so two rows list the features they share in the same order.
pub const PUBLISHED_FEATURE_VALUES: &[&str] = &[
    FEATURE_BATCH_INFERENCE,
    FEATURE_INTELLIGENT_PROMPT_ROUTING,
    FEATURE_LATENCY_OPTIMIZATION,
    FEATURE_PROMPT_CACHING,
];

/// `featuresSupported` — no feature's table lists the model, which is where a freshly
/// offered model starts.
const NO_FEATURES: &[&str] = &[];
/// `featuresSupported` — a model batch inference runs and no other feature lists.
const BATCH_ONLY: &[&str] = &[FEATURE_BATCH_INFERENCE];
/// `featuresSupported` — a model batch inference runs and a prompt router routes to.
const BATCH_AND_ROUTING: &[&str] = &[FEATURE_BATCH_INFERENCE, FEATURE_INTELLIGENT_PROMPT_ROUTING];
/// `featuresSupported` — a model every one of these four features lists.
const EVERY_PUBLISHED_FEATURE: &[&str] = PUBLISHED_FEATURE_VALUES;
/// `FoundationModelLifecycleStatus` — the model is available.
const LIFECYCLE_ACTIVE: &str = "ACTIVE";
/// `FoundationModelLifecycleStatus` — the model is deprecated: it is still offered and
/// still invocable, and callers are expected to move off it before its end of life. The
/// two values this enum carries are the two states an OFFERED model can be in; a model
/// past its end of life is no longer offered, so it is absent from this catalog rather
/// than carried in it under a status (see [`FOUNDATION_MODELS`]).
const LIFECYCLE_LEGACY: &str = "LEGACY";

/// Every foundation model this emulator's Amazon Bedrock offers, in `modelId` order.
///
/// Amazon Bedrock publishes its base models rather than letting a caller create one —
/// there is no `CreateFoundationModel` — so this is the published catalog itself: the set
/// `ListFoundationModels` reports and `GetFoundationModel` resolves one row of, not
/// anything a request can add to or remove from.
///
/// It is the set of models the service currently OFFERS, which is what makes it the set
/// both of those operations answer out of: a model is here while it is available
/// ([`LIFECYCLE_ACTIVE`]) and while it is deprecated ([`LIFECYCLE_LEGACY`]), and a model
/// past its end of life is not offered at all, so it is in no row — it is listed by
/// nothing and resolves for nobody. The inference plane keeps such an id in its own
/// inventory to refuse it, which is where the end-of-life refusal belongs; the control
/// plane simply no longer carries the model.
///
/// The ids are the base model ids AWS publishes in "Amazon Bedrock base model IDs
/// (on-demand throughput)" and "Supported foundation models in Amazon Bedrock", and the
/// rows AWS marks deprecated carry the status "Model lifecycle" lists them under; this
/// emulator's bedrock-runtime plane serves the same ids, so the two planes agree about
/// which models exist. Each row's provider, family, modalities, streaming support,
/// customizations, inference types and published features are as those pages publish them
/// for that model — the features from each feature's own "Supported models" table, so a
/// model a table does not list does not support that feature — and a per-model attribute no
/// page publishes is left out of the row rather than invented (`publicExtendedAccessTime`
/// is such an instant, so no row carries one).
///
/// It is a SUBSET of what AWS offers, spanning every value of each member a caller can
/// filter on, because the filter rule — not the catalog's size — is what this emulator
/// has to get right.
pub const FOUNDATION_MODELS: &[FoundationModel] = &[
    FoundationModel {
        model_id: "ai21.jamba-1-5-mini-v1:0",
        model_name: "Jamba 1.5 Mini",
        provider_name: "AI21 Labs",
        model_family: "Jamba 1.5",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-08-22T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-canvas-v1:0",
        model_name: "Nova Canvas",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-lite-v1:0",
        model_name: "Nova Lite",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: EVERY_PUBLISHED_FEATURE,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-micro-v1:0",
        model_name: "Nova Micro",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: EVERY_PUBLISHED_FEATURE,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-pro-v1:0",
        model_name: "Nova Pro",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: EVERY_PUBLISHED_FEATURE,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.titan-embed-image-v1",
        model_name: "Titan Embed Image",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.titan-embed-text-v2:0",
        model_name: "Titan Embed Text V2",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-30T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.titan-image-generator-v1",
        model_name: "Titan Image Gen G1",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.titan-text-lite-v1",
        model_name: "Titan Text Lite",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_PRE_TRAINING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-04-30T00:00:00Z",
        }),
    },
    FoundationModel {
        model_id: "amazon.titan-text-premier-v1:0",
        model_name: "Titan Text Premier",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-30T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        model_name: "Claude 3.5 Sonnet",
        provider_name: "Anthropic",
        model_family: "Claude 3.5",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_AND_ROUTING,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-10-22T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        model_name: "Claude 3 Haiku",
        provider_name: "Anthropic",
        model_family: "Claude 3",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_AND_ROUTING,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-03-13T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
        model_name: "Claude 3 Sonnet",
        provider_name: "Anthropic",
        model_family: "Claude 3",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-03-04T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "cohere.command-light-text-v14",
        model_name: "Command Light",
        provider_name: "Cohere",
        model_family: "Command",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-06-30T00:00:00Z",
        }),
    },
    FoundationModel {
        model_id: "cohere.command-text-v14",
        model_name: "Command",
        provider_name: "Cohere",
        model_family: "Command",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-06-30T00:00:00Z",
        }),
    },
    FoundationModel {
        model_id: "cohere.embed-english-v3",
        model_name: "Embed English",
        provider_name: "Cohere",
        model_family: "Embed",
        input_modalities: IN_TEXT,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "meta.llama3-1-8b-instruct-v1:0",
        model_name: "Llama 3.1 8B",
        provider_name: "Meta",
        model_family: "Llama 3.1",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-07-23T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "meta.llama3-70b-instruct-v1:0",
        model_name: "Llama 3 70B Instruct",
        provider_name: "Meta",
        model_family: "Llama 3",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-18T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "mistral.mistral-large-2402-v1:0",
        model_name: "Mistral Large",
        provider_name: "Mistral AI",
        model_family: "Mistral Large",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-02T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "mistral.mixtral-8x7b-instruct-v0:1",
        model_name: "Mixtral 8x7B",
        provider_name: "Mistral AI",
        model_family: "Mixtral",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-03-01T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "moonshotai.kimi-k2.5",
        model_name: "Kimi K2.5",
        provider_name: "Moonshot AI",
        model_family: "Kimi K2.5",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2025-12-09T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "stability.stable-diffusion-xl-v1",
        model_name: "SDXL 1.0",
        provider_name: "Stability AI",
        model_family: "SDXL",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
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

    /// The model a QUALIFIED base model id names: a `modelId` the catalog carries, or one
    /// of its ids followed by the `:`-separated version qualifiers AWS appends when it
    /// writes a base model id into a resource ARN (`…:0:8k`).
    ///
    /// Those qualifiers are part of the id a `CustomModelArn` embeds — its pattern admits
    /// `([:][a-z0-9-]{1,63}){0,2}` after the model id
    /// (`smithy_query(query="operation",operation="CreateCustomModelDeployment")`) — and
    /// they name a variant of the SAME published model, so the qualified id resolves to
    /// the catalog row its unqualified prefix names. Matching is at a `:` boundary only,
    /// so one model id is never read as a prefix of another.
    #[must_use]
    pub fn qualified_model(&self, qualified_model_id: &str) -> Option<&FoundationModel> {
        let mut candidate = qualified_model_id;
        loop {
            if let Some(model) = self.model(candidate) {
                return Some(model);
            }
            candidate = candidate.rsplit_once(':')?.0;
        }
    }

    /// What a `ModelIdentifier` naming a BASE model names: a model this account's catalog
    /// offers, a model this service still recognises and offers no longer
    /// ([`RETIRED_FOUNDATION_MODELS`]), or no model at all.
    ///
    /// Both spellings the member admits are read — the bare model id and the model's
    /// `foundation-model` ARN ([`base_model_id_of`]) — and the id is resolved the way a base
    /// model id embedded in a resource is ([`Self::qualified_model`]), so a `:`-qualified
    /// variant names the published model it is a variant of.
    ///
    /// The three answers are what let an operation refuse a model it cannot act on for the
    /// right REASON: a model that exists but may not back this request is an eligibility or
    /// authorization decision of that operation, while only an identifier naming no model at
    /// all is a missing resource.
    #[must_use]
    pub fn resolve_base_model(&self, model_identifier: &str) -> BaseModelIdentity<'_> {
        let base_model_id = base_model_id_of(model_identifier);
        if let Some(model) = self.qualified_model(base_model_id) {
            return BaseModelIdentity::Offered(model);
        }
        if RETIRED_FOUNDATION_MODELS.contains(&base_model_id) {
            return BaseModelIdentity::Retired;
        }
        BaseModelIdentity::Unknown
    }
}

/// Whether this account reaches every model the catalog offers without asking for access to
/// one model at a time.
///
/// Reported beside the listing itself, because it is what tells a client whether a model it
/// found in the catalog needs an access request before it can be invoked, or is simply
/// there to be used. Account-level like [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`],
/// and PUBLISHED rather than created: no operation of this service turns it off, so it
/// seeds itself the way the catalog does ([`FoundationModelCatalog::default`]) and a read
/// of it changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundationModelAccessDefault(bool);

impl Default for FoundationModelAccessDefault {
    fn default() -> Self {
        Self(ACCESS_TO_ALL_MODELS_BY_DEFAULT)
    }
}

impl FoundationModelAccessDefault {
    /// Whether access to all models is on for this account.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.0
    }
}

/// An account reaches the models this control plane offers without requesting access to
/// each one: the models that need a provider agreement accepted first say so through their
/// own offer ([`model_agreement_outstanding`]) rather than through this flag, so the flag
/// is the account-level default and not a per-model one.
pub const ACCESS_TO_ALL_MODELS_BY_DEFAULT: bool = true;

/// What a `modelId` naming a base model resolves to
/// ([`FoundationModelCatalog::resolve_base_model`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseModelIdentity<'a> {
    /// A model the published catalog offers: the row itself.
    Offered(&'a FoundationModel),
    /// A model this service recognises and no longer offers, so there is no row to read and
    /// nothing it is offered under.
    Retired,
    /// No model of this service: the identifier resolves to nothing at all.
    Unknown,
}

/// The base model ids this service still RECOGNISES although it offers the model no longer:
/// the ids AWS publishes as past their end of life ("Model lifecycle" in the Amazon Bedrock
/// User Guide).
///
/// A model past its end of life is OFFERED by nobody, which is why [`FOUNDATION_MODELS`]
/// carries no row for one — no listing reports it and no read of the catalog resolves it.
/// Its id goes on naming a model this service knows, though, so a request that names one is
/// not a request whose identifier cannot be read: an operation deciding whether a model may
/// back what it is being asked for refuses it on THAT ground — the model is offered under
/// nothing — rather than reporting that the identifier names no model
/// ([`FoundationModelCatalog::resolve_base_model`]). This emulator's bedrock-runtime plane
/// keeps the same ids in its own inventory for the same reason, where they are answered with
/// the end-of-life refusal rather than an unreadable-identifier one.
pub const RETIRED_FOUNDATION_MODELS: &[&str] = &[
    "ai21.j2-grande-instruct",
    "ai21.j2-jumbo-instruct",
    "ai21.j2-mid-v1",
    "ai21.j2-ultra-v1",
    "amazon.titan-text-express-v1",
    "anthropic.claude-instant-v1",
    "anthropic.claude-v1",
    "anthropic.claude-v2",
    "anthropic.claude-v2:1",
    "meta.llama2-13b-chat-v1",
    "meta.llama2-70b-chat-v1",
    "stability.stable-diffusion-xl-v0",
];

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

// =============================================================================
// The capability projection of a published model
// =============================================================================
//
// Amazon Bedrock reports, beside the ten members `FoundationModelSummary` declares, the
// CAPABILITIES of the model: the family it belongs to, the invocation APIs it is reachable
// through, and whether guardrails, prompt caching, intelligent prompt routing, latency
// optimization and batch inference may be used with it. None of them is declared on the
// on-disk shape (`smithy_query(query="shape", shape="FoundationModelSummary")` lists
// modelArn/modelId/modelName/providerName/inputModalities/outputModalities/
// responseStreamingSupported/customizationsSupported/inferenceTypesSupported/
// modelLifecycle), so they are appended after it — a client decides from them whether it
// may stream, converse, guard, cache, route, batch or customise a model it picked out of
// the listing, which is what makes a listing that omits them undecidable.
//
// Each one is read off the catalog row: the family and the four features of
// [`PUBLISHED_FEATURE_VALUES`] are PUBLISHED per model — each feature has its own
// "Supported models" table, so support for it is a fact about the model and not a
// consequence of what the model does — while the rest follow from attributes the row
// already carries, so they are computed rather than stored a second time. Nothing here is
// invented for a model: a feature no table lists the model in is simply not supported for
// it, which is where a freshly offered model starts.

/// `ModelModality` — the modality a model that takes part in a conversation produces
/// (`crate::ops::types::MODEL_MODALITY_VALUES`).
const MODALITY_TEXT: &str = "TEXT";

/// The `providerName` whose own OpenAI-compatible endpoints Amazon Bedrock serves its
/// models on beside its own invocation APIs, which is what puts those two APIs in their
/// `inferenceAPIsSupported`.
const OPENAI_MODEL_PROVIDER: &str = "OpenAI";

/// The Converse API: Amazon Bedrock's own conversation API.
const INFERENCE_API_CONVERSE: &str = "converse";
/// The InvokeModel API: every offered model is reachable through it, whatever it produces.
const INFERENCE_API_INVOKE_MODEL: &str = "invokeModel";
/// The OpenAI-compatible chat completions endpoint.
const INFERENCE_API_OPENAI_CHAT_COMPLETIONS: &str = "openAiChatCompletions";
/// The OpenAI-compatible responses endpoint.
const INFERENCE_API_OPENAI_RESPONSES: &str = "openAiResponses";

/// `featuresSupported` — a model invocation job may run the model over a dataset ("Supported
/// models and Regions for batch inference").
const FEATURE_BATCH_INFERENCE: &str = "BATCH_INFERENCE";
/// `featuresSupported` — a prompt router may route a prompt to the model ("Supported models
/// and Regions for intelligent prompt routing").
const FEATURE_INTELLIGENT_PROMPT_ROUTING: &str = "INTELLIGENT_PROMPT_ROUTING";
/// `featuresSupported` — the model is offered with latency-optimized inference ("Supported
/// models and Regions for latency-optimized inference").
const FEATURE_LATENCY_OPTIMIZATION: &str = "LATENCY_OPTIMIZATION";
/// `featuresSupported` — a prompt prefix may be cached explicitly for the model ("Supported
/// models and Regions for prompt caching").
const FEATURE_PROMPT_CACHING: &str = "PROMPT_CACHING";

/// Whether `model` takes part in a conversation: it produces TEXT, which is the one
/// modality a turn of a conversation is made of.
///
/// The precondition of every capability that acts on what a model SAYS — Converse and
/// guardrails — so an embedding or image model, which says nothing a further turn can be
/// built from, has neither.
#[must_use]
pub fn model_is_conversational(model: &FoundationModel) -> bool {
    model.output_modalities.contains(&MODALITY_TEXT)
}

/// Whether a guardrail may be applied to `model`.
///
/// A guardrail evaluates the text a model is given and the text it produces, so it applies
/// to a model that produces text and to no other: there is nothing in a vector or an image
/// for a topic, word or contextual-grounding policy to decide about. Unlike the four
/// features of [`PUBLISHED_FEATURE_VALUES`], this needs nothing of the model beyond that —
/// a guardrail is applied BESIDE the invocation rather than built into the model's
/// integration — so it is decided here rather than published per model.
#[must_use]
pub fn model_guardrails_supported(model: &FoundationModel) -> bool {
    model_is_conversational(model)
}

/// Whether `feature` — one of [`PUBLISHED_FEATURE_VALUES`] — is published for `model`.
fn model_publishes_feature(model: &FoundationModel, feature: &str) -> bool {
    model.features_supported.contains(&feature)
}

/// Whether a prompt prefix may be cached explicitly for `model`.
#[must_use]
pub fn model_explicit_prompt_caching(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_PROMPT_CACHING)
}

/// Whether a prompt router may route a prompt to `model`.
#[must_use]
pub fn model_intelligent_prompt_routing(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_INTELLIGENT_PROMPT_ROUTING)
}

/// Whether `model` is offered with latency-optimized inference.
#[must_use]
pub fn model_latency_optimization_supported(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_LATENCY_OPTIMIZATION)
}

/// Whether a model invocation job may run `model` over a dataset.
#[must_use]
pub fn model_batch_supported(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_BATCH_INFERENCE)
}

/// The invocation APIs `model` is reachable through, in the order this control plane
/// reports them.
///
/// `invokeModel` reaches every offered model, whatever it produces; `converse` carries a
/// CONVERSATION, so it reaches a conversational model ([`model_is_conversational`]) and no
/// other; and the two OpenAI-compatible endpoints reach a model published by that provider
/// ([`OPENAI_MODEL_PROVIDER`]), whose own API they are the shape of.
#[must_use]
pub fn model_inference_apis_supported(model: &FoundationModel) -> Vec<String> {
    let mut apis: Vec<String> = Vec::new();
    if model_is_conversational(model) {
        apis.push(INFERENCE_API_CONVERSE.to_string());
    }
    apis.push(INFERENCE_API_INVOKE_MODEL.to_string());
    if model.provider_name == OPENAI_MODEL_PROVIDER {
        apis.push(INFERENCE_API_OPENAI_CHAT_COMPLETIONS.to_string());
        apis.push(INFERENCE_API_OPENAI_RESPONSES.to_string());
    }
    apis
}

/// The features published for `model` ([`FoundationModel::features_supported`]), as the one
/// list a client can test membership in.
///
/// The same set the four feature members beside it report one by one, so the list and those
/// members cannot disagree: a feature is listed exactly while the row publishes it.
#[must_use]
pub fn model_features_supported(model: &FoundationModel) -> Vec<String> {
    model
        .features_supported
        .iter()
        .map(|feature| (*feature).to_string())
        .collect()
}

/// What the Converse API supports for `model`.
///
/// Reported beside `inferenceAPIsSupported` because Converse is the API a conversational
/// client drives the model through, and the two things it has to know before it does are
/// whether the model is reachable that way at all and whether the answer can be streamed —
/// the model's own `responseStreamingSupported`, which it only means anything to stream
/// through Converse when the model is conversational.
#[must_use]
pub fn model_converse_support(model: &FoundationModel) -> ConverseSupport {
    let supported = model_is_conversational(model);
    ConverseSupport {
        supported: Some(supported),
        streaming_supported: Some(supported && model.response_streaming_supported),
    }
}

/// One catalog row as the `FoundationModelSummary` a `ListFoundationModels` response for
/// `region` carries.
///
/// Lives here rather than in `ops/list_foundation_models.rs` because every member it
/// carries is also a member `GetFoundationModel` reports
/// ([`foundation_model_details`], which is built out of it), and the two must agree about
/// them: a listed summary is the same row the read of that one model finds.
///
/// `modelLifecycle` reports the row's `status`, the row's launch instant
/// ([`FoundationModel::start_of_life_time`], which every offered model has) and, for a
/// deprecated model, both ends of the legacy window AWS publishes for it
/// ([`FoundationModel::legacy_window`]): when it entered that state and when it stops being
/// available. `publicExtendedAccessTime` is left out, because nothing publishes it per
/// model: it is part of a legacy window AWS states no instant for.
///
/// The members after `modelLifecycle` are the model's CAPABILITIES, each computed from this
/// row — see the capability section above for what each one takes of a model.
#[must_use]
pub fn foundation_model_summary(model: &FoundationModel, region: &str) -> FoundationModelSummary {
    let list = |values: &[&str]| values.iter().map(|value| (*value).to_string()).collect();
    FoundationModelSummary {
        model_arn: foundation_model_arn(region, model.model_id),
        model_id: model.model_id.to_string(),
        model_name: Some(model.model_name.to_string()),
        provider_name: Some(model.provider_name.to_string()),
        input_modalities: Some(list(model.input_modalities)),
        output_modalities: Some(list(model.output_modalities)),
        response_streaming_supported: Some(model.response_streaming_supported),
        customizations_supported: Some(list(model.customizations_supported)),
        inference_types_supported: Some(list(model.inference_types_supported)),
        model_lifecycle: Some(FoundationModelLifecycle {
            status: model.lifecycle_status.to_string(),
            start_of_life_time: published_instant(model.start_of_life_time),
            end_of_life_time: model
                .legacy_window
                .and_then(|window| published_instant(window.end_of_life_at)),
            legacy_time: model
                .legacy_window
                .and_then(|window| published_instant(window.entered_at)),
            public_extended_access_time: None,
        }),
        model_family: Some(model.model_family.to_string()),
        inference_apis_supported: Some(model_inference_apis_supported(model)),
        features_supported: Some(model_features_supported(model)),
        guardrails_supported: Some(model_guardrails_supported(model)),
        explicit_prompt_caching: Some(model_explicit_prompt_caching(model)),
        intelligent_prompt_routing: Some(model_intelligent_prompt_routing(model)),
        latency_optimization_supported: Some(model_latency_optimization_supported(model)),
        batch_supported: Some(model_batch_supported(model)),
        converse: Some(model_converse_support(model)),
    }
}

/// A PUBLISHED instant of the catalog — one written in this file as the RFC 3339 day AWS
/// publishes for it — as the wire timestamp type the response shapes carry.
///
/// `None` only for a value that is not an instant at all, which no row of
/// [`FOUNDATION_MODELS`] carries: a row's instants are pinned by this crate's own suites, so
/// a misspelled one fails there rather than being reported as an absent date.
fn published_instant(published: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(published, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The same catalog row as the `FoundationModelDetails` a `GetFoundationModel` response
/// for `region` carries.
///
/// `FoundationModelDetails` and `FoundationModelSummary` carry the same members, and
/// a read of a model must report what a listing reports for it, so this moves the ONE
/// mapping [`foundation_model_summary`] computes into the shape this response declares
/// rather than computing the members a second time.
#[must_use]
pub fn foundation_model_details(model: &FoundationModel, region: &str) -> FoundationModelDetails {
    let summary = foundation_model_summary(model, region);
    FoundationModelDetails {
        model_arn: summary.model_arn,
        model_id: summary.model_id,
        model_name: summary.model_name,
        provider_name: summary.provider_name,
        input_modalities: summary.input_modalities,
        output_modalities: summary.output_modalities,
        response_streaming_supported: summary.response_streaming_supported,
        customizations_supported: summary.customizations_supported,
        inference_types_supported: summary.inference_types_supported,
        model_lifecycle: summary.model_lifecycle,
        model_family: summary.model_family,
        inference_apis_supported: summary.inference_apis_supported,
        features_supported: summary.features_supported,
        guardrails_supported: summary.guardrails_supported,
        explicit_prompt_caching: summary.explicit_prompt_caching,
        intelligent_prompt_routing: summary.intelligent_prompt_routing,
        latency_optimization_supported: summary.latency_optimization_supported,
        batch_supported: summary.batch_supported,
        converse: summary.converse,
    }
}

// =============================================================================
// The agreement offers of a published model
// =============================================================================

/// `OfferType` — every offer associated with the model.
pub const OFFER_TYPE_ALL: &str = "ALL";

/// `OfferType` — only the offers that are publicly available, as against an offer agreed
/// privately with a provider.
pub const OFFER_TYPE_PUBLIC: &str = "PUBLIC";

/// The `providerName` of the models AWS itself publishes.
///
/// It is the one provider whose models an account reaches without accepting another
/// party's terms, which is what [`model_agreement_offers`] reads it for.
pub const AWS_MODEL_PROVIDER: &str = "Amazon";

/// The offers associated with `model`, narrowed to `offer_type` when the request names one.
///
/// A model from a THIRD-PARTY provider is offered under that provider's own end-user
/// license agreement, which the account accepts — by the `offerToken` of one of these
/// offers — before it may use the model; the models AWS publishes itself
/// ([`AWS_MODEL_PROVIDER`]) carry no such agreement, so they expose no offer and the
/// listing for one is an empty list rather than a refusal.
///
/// Every offer here is PUBLIC: this emulator holds no privately agreed offer, so
/// [`OFFER_TYPE_ALL`] and [`OFFER_TYPE_PUBLIC`] admit the same one offer per model, and a
/// request naming no `offerType` gets every offer the model has.
///
/// The TERMS a model is offered on are the provider's published document, price list and
/// support policy, none of which this emulator carries — and every member of `LegalTerm`,
/// `SupportTerm`, `ValidityTerm` and `DimensionalPriceRate` is optional
/// (`smithy_query(query="shape",shape="TermDetails")`) — so the term structures an offer
/// carries are present and name no document, price or duration rather than inventing one.
#[must_use]
pub fn model_agreement_offers(model: &FoundationModel, offer_type: Option<&str>) -> Vec<Offer> {
    if model.provider_name == AWS_MODEL_PROVIDER {
        return Vec::new();
    }
    // The one offer a third-party model is published under is PUBLIC, so it is admitted by
    // either value the filter can carry, and by a request that carries no filter at all.
    let admitted = offer_type
        .is_none_or(|wanted| wanted == OFFER_TYPE_ALL || wanted == OFFER_TYPE_PUBLIC);
    if !admitted {
        return Vec::new();
    }
    let offer_id = model_offer_id(model.model_id);
    vec![Offer {
        offer_token: model_offer_token(model.model_id, &offer_id),
        offer_id: Some(offer_id),
        term_details: TermDetails {
            usage_based_pricing_term: PricingTerm {
                rate_card: Vec::new(),
            },
            legal_term: LegalTerm { url: None },
            support_term: SupportTerm {
                refund_policy_description: None,
            },
            validity_term: None,
        },
    }]
}

/// Whether this account has still to accept an agreement before it may use `model`.
///
/// An OUTSTANDING offer is exactly that statement: a model published under its provider's
/// own end-user license agreement is offered to the account until the account accepts those
/// terms, while a model AWS publishes itself is offered under none and so has no offer to
/// accept ([`model_agreement_offers`]). Read off the same offers a listing reports, so the
/// agreement a read of the model's availability describes is the one a caller can act on.
#[must_use]
pub fn model_agreement_outstanding(model: &FoundationModel) -> bool {
    !model_agreement_offers(model, None).is_empty()
}

/// `AgreementStatus` — the agreement this account needs for the model is in place, which
/// includes a model that needs none.
pub const AGREEMENT_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `AgreementStatus` — the account holds no agreement for the model, so an offer of the
/// model's terms is still outstanding ([`model_agreement_outstanding`]).
pub const AGREEMENT_STATUS_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// `AuthorizationStatus` — the account may use the model.
pub const AUTHORIZATION_STATUS_AUTHORIZED: &str = "AUTHORIZED";

/// `AuthorizationStatus` — the account may not use the model, because the terms it is
/// offered under have not been accepted.
pub const AUTHORIZATION_STATUS_NOT_AUTHORIZED: &str = "NOT_AUTHORIZED";

/// `EntitlementAvailability` and `RegionAvailability` both spell their positive value this
/// way: the entitlement the account would use the model under exists, and the model is
/// offered in the region the request addressed.
pub const AVAILABILITY_AVAILABLE: &str = "AVAILABLE";

/// The same value set's negative: no entitlement, or no offering in the region.
pub const AVAILABILITY_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// The `offerId` of the public offer of `model_id`.
///
/// Derived from the model the offer is FOR, so the id a listing reports for a model is the
/// same id every later listing reports for it: an offer is not a resource a request brings
/// into being, so nothing about it is per-run.
#[must_use]
pub fn model_offer_id(model_id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "offer-{}",
        &hex::encode(Sha256::digest(model_id.as_bytes()))[..12]
    )
}

/// The `offerToken` of the offer `offer_id` of `model_id`: the opaque token
/// `CreateFoundationModelAgreement` takes to accept exactly this offer.
///
/// Derived from what the offer IS — the model and the offer id — for the reason
/// [`model_offer_id`] is: the token a listing hands out has to be the token a later
/// agreement call can be checked against, so it is computed from the offer rather than
/// minted per run.
#[must_use]
pub fn model_offer_token(model_id: &str, offer_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(model_id.as_bytes());
    digest.update(b":");
    digest.update(offer_id.as_bytes());
    hex::encode(digest.finalize())
}

/// What this control plane answers when a `modelId` names no model in the catalog: the
/// identifier could not be resolved to a model.
///
/// Stated once here because every operation that resolves a `BedrockModelId` against the
/// published catalog makes the same refusal for the same reason, and two spellings of it
/// could drift.
#[must_use]
pub fn foundation_model_not_found(identifier: &str) -> String {
    format!("Could not resolve the model from the provided model identifier: {identifier}")
}

/// Why a `modelIdentifier` names no model the catalog offers, and so WHICH refusal the read
/// of it answers.
///
/// Three different faults hide behind "the catalog has no row for it", and Amazon Bedrock
/// tells them apart rather than reporting one of them for all three:
///
/// * a model the service recognises and no longer offers is a resource that EXISTED and is
///   gone: `ResourceNotFoundException` (404), carrying the end-of-life reason;
/// * an ARN naming no model is an ARN to report missing: `ResourceNotFoundException` (404),
///   which is what that error documents itself as ("The specified resource Amazon Resource
///   Name (ARN) was not found");
/// * a bare model id naming no model is a value this service cannot read as a model at all,
///   which is an INPUT fault: `ValidationException` (400) carrying the fixed
///   [`INVALID_MODEL_IDENTIFIER_MESSAGE`].
///
/// Keeping them apart is what lets a caller act on the answer: 400 says "fix the request",
/// where 404 is routinely retried as eventual consistency. It is the same split the
/// inference plane makes for the same member, read here off the published catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnofferedFoundationModel {
    /// A model past its end of life ([`RETIRED_FOUNDATION_MODELS`]).
    PastEndOfLife,
    /// A `foundation-model` ARN of this service's namespace naming no model.
    MissingArn,
    /// A bare model id naming no model.
    UnreadableIdentifier,
}

impl UnofferedFoundationModel {
    /// The sentence this refusal carries. Fixed text for the two readings whose fault is
    /// the CLASS of the identifier rather than the value in it; the missing ARN echoes the
    /// ARN the request addressed, because that is the resource it reports absent.
    #[must_use]
    pub fn message(self, identifier: &str) -> String {
        match self {
            Self::PastEndOfLife => FOUNDATION_MODEL_END_OF_LIFE_MESSAGE.to_string(),
            Self::MissingArn => foundation_model_not_found(identifier),
            Self::UnreadableIdentifier => INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
        }
    }
}

/// How `identifier` — a `modelIdentifier` the catalog resolved to no offered model
/// ([`FoundationModelCatalog::resolve_base_model`]) — is refused.
///
/// Stated once because the reading belongs to the published CATALOG rather than to one
/// operation: every read that resolves a model identifier against it refuses an identifier
/// it cannot resolve the same way.
#[must_use]
pub fn unoffered_foundation_model(identifier: &str) -> UnofferedFoundationModel {
    if RETIRED_FOUNDATION_MODELS.contains(&base_model_id_of(identifier)) {
        UnofferedFoundationModel::PastEndOfLife
    } else if names_arn_of(identifier, FOUNDATION_MODEL_RESOURCE_TYPE) {
        UnofferedFoundationModel::MissingArn
    } else {
        UnofferedFoundationModel::UnreadableIdentifier
    }
}

/// The resource type a `foundation-model` ARN names, as `FoundationModelArn`'s pattern
/// spells it (`…::foundation-model/<modelId>`) — what tells that spelling of a
/// `modelIdentifier` from the bare model id the same member admits ([`names_arn_of`]).
pub const FOUNDATION_MODEL_RESOURCE_TYPE: &str = "foundation-model";

/// What this control plane answers for a model id whose model has reached end of life.
///
/// Taken from the Bedrock Runtime emulator, which states the same refusal for the same
/// class on the inference surface, rather than spelled a second time here: one wording for
/// one sentence, exactly as [`INVALID_MODEL_IDENTIFIER_MESSAGE`] is shared.
pub const FOUNDATION_MODEL_END_OF_LIFE_MESSAGE: &str =
    ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::END_OF_LIFE_MESSAGE;

// =============================================================================
// The published default prompt router catalog
// =============================================================================
//
// Amazon Bedrock publishes the `default` prompt routers itself — `PromptRouterType` is
// `[custom, default]` and `ListPromptRouters$type` DEFAULTS to `default`
// (`smithy_query(query="operation",operation="ListPromptRouters")`) — and there is no
// operation that creates one, so these rows are published data the same way
// [`FOUNDATION_MODELS`] is: the same for every account, written by no handler. A caller
// discovers the routable model sets from them before building a router of its own.

/// `PromptRouterType` — the type of a router Amazon Bedrock itself publishes.
///
/// The other of the two values the enum carries, beside [`PROMPT_ROUTER_TYPE_CUSTOM`].
pub const PROMPT_ROUTER_TYPE_DEFAULT: &str = "default";

/// The `responseQualityDifference` a published router reports.
///
/// `RoutingCriteria$responseQualityDifference` is modelled `@range(min: 0, max: 100)` and
/// is `@required`, so a published router reports one; AWS publishes no tolerance for its
/// own routers, so this is the minimum that range admits — the strictest criteria, under
/// which a prompt is routed to the smaller model only when its answer is no worse.
pub const PUBLISHED_ROUTING_QUALITY_DIFFERENCE: f64 = 0.0;

/// One prompt router Amazon Bedrock publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultPromptRouter {
    /// The resource half of the router's `PromptRouterArn`, whose pattern admits
    /// `[a-zA-Z0-9-:.]+` there ([`prompt_router_arn`]).
    pub id: &'static str,
    /// `promptRouterName` — inside `PromptRouterName`'s `^([0-9a-zA-Z][ _-]?)+$`.
    pub name: &'static str,
    /// The `modelId`s the router routes between, in the order the summary reports them.
    /// Every id is a row of [`FOUNDATION_MODELS`], so a router never offers a model this
    /// control plane does not serve.
    pub models: &'static [&'static str],
    /// The `modelId` of the model the router falls back to when the criteria are not met;
    /// one of [`DefaultPromptRouter::models`].
    pub fallback_model: &'static str,
}

/// The prompt routers this emulator's Amazon Bedrock publishes, in `id` order — the order
/// their ARNs sort in, which is the order a listing pages through.
///
/// The ids, names and model sets are the ones AWS publishes in "Intelligent prompt
/// routing": one router per model family, routing between two models of that family and
/// falling back to the larger one. The model ids are the rows [`FOUNDATION_MODELS`]
/// carries for those families, so the two catalogs agree about which models exist.
///
/// Offered in every region this emulator serves: AWS publishes these routers in the
/// regions where intelligent prompt routing is available, and nothing in the model marks a
/// region as one of them, so a region is not treated as having none.
pub const DEFAULT_PROMPT_ROUTERS: &[DefaultPromptRouter] = &[
    DefaultPromptRouter {
        id: "anthropic.claude:1",
        name: "Anthropic Prompt Router",
        models: &[
            "anthropic.claude-3-haiku-20240307-v1:0",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
        ],
        fallback_model: "anthropic.claude-3-5-sonnet-20241022-v2:0",
    },
    DefaultPromptRouter {
        id: "meta.llama:1",
        name: "Meta Prompt Router",
        models: &[
            "meta.llama3-1-8b-instruct-v1:0",
            "meta.llama3-70b-instruct-v1:0",
        ],
        fallback_model: "meta.llama3-70b-instruct-v1:0",
    },
];

/// Every prompt router Amazon Bedrock publishes in `region`, as the `PromptRouterSummary`
/// a listing carries, in `promptRouterArn` order.
///
/// Each one reports the foundation models it routes between as `PromptRouterTargetModel`s
/// carrying the `FoundationModelArn` of this region ([`foundation_model_arn`]) — the form
/// `PromptRouterTargetModelArn` admits — and no `createdAt` or `updatedAt`: those two
/// members are optional, and nothing publishes an instant for a router no caller created.
#[must_use]
pub fn default_prompt_routers(region: &str) -> Vec<PromptRouterSummary> {
    let target = |model_id: &str| PromptRouterTargetModel {
        model_arn: foundation_model_arn(region, model_id),
    };
    DEFAULT_PROMPT_ROUTERS
        .iter()
        .map(|router| PromptRouterSummary {
            prompt_router_name: router.name.to_string(),
            routing_criteria: RoutingCriteria {
                response_quality_difference: PUBLISHED_ROUTING_QUALITY_DIFFERENCE,
            },
            description: None,
            created_at: None,
            updated_at: None,
            prompt_router_arn: prompt_router_arn(region, router.id),
            models: router.models.iter().map(|model| target(model)).collect(),
            fallback_model: target(router.fallback_model),
            status: PROMPT_ROUTER_AVAILABLE.to_string(),
            r#type: PROMPT_ROUTER_TYPE_DEFAULT.to_string(),
        })
        .collect()
}

/// The prompt router Amazon Bedrock publishes in `region` under `arn`, if it publishes one.
///
/// `GetPromptRouter` addresses a router by its `PromptRouterArn` alone, and a published
/// router carries the ARN of the region it is offered in, so one of another region names
/// another account-region's router.
#[must_use]
pub fn default_prompt_router(arn: &str, region: &str) -> Option<PromptRouterSummary> {
    default_prompt_routers(region)
        .into_iter()
        .find(|published| published.prompt_router_arn == arn)
}

// =============================================================================
// The published cross-region inference profile catalog
// =============================================================================
//
// Amazon Bedrock publishes the SYSTEM_DEFINED inference profiles itself — "the inference
// profile is defined by Amazon Bedrock. You can route inference requests across regions
// with these inference profiles"
// (`smithy_query(query="shape",shape="InferenceProfileSummary")`) — and there is no
// operation that creates one, so these rows are published data the same way
// [`FOUNDATION_MODELS`] is: the same for every account, written by no handler.

/// `InferenceProfileType` — the type of a profile Amazon Bedrock itself defines.
///
/// The other of the two values `InferenceProfileType` carries, beside
/// [`INFERENCE_PROFILE_TYPE_APPLICATION`].
pub const INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED: &str = "SYSTEM_DEFINED";

/// One geography a cross-region inference profile routes invocations within.
///
/// A geography is the unit the published profiles are cut along: a profile exists once per
/// geography per model, is offered in that geography's regions, and routes requests to
/// those same regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceProfileGeography {
    /// The leading dot-separated segment of the published `inferenceProfileId`
    /// (`us.<modelId>`), which is also what `ops/create_inference_profile.rs` reads back
    /// off such an id to find the model behind it.
    pub prefix: &'static str,
    /// How the published `inferenceProfileName` spells the geography. Spelled without a
    /// `.` so a published name stays inside `InferenceProfileName`'s pattern
    /// (`^([0-9a-zA-Z][ _-]?)+$`).
    pub label: &'static str,
    /// The regions of the geography: where the profile is offered, and where it routes
    /// invocations to. At most five, because `InferenceProfileModels` is
    /// `@length(min: 1, max: 5)` and a published profile reports one model per region.
    pub regions: &'static [&'static str],
}

/// The geographies this emulator's Amazon Bedrock publishes cross-region profiles for, in
/// `prefix` order — the order their profile ids sort in.
///
/// The prefixes and the regions are the ones AWS publishes in "Supported Regions and
/// models for inference profiles": a geography's profile routes to the commercial regions
/// of that geography. A SUBSET of what AWS offers, spanning geographies of three different
/// sizes, because the listing rule — not the catalog's size — is what this emulator has to
/// get right.
pub const INFERENCE_PROFILE_GEOGRAPHIES: &[InferenceProfileGeography] = &[
    InferenceProfileGeography {
        prefix: "apac",
        label: "APAC",
        regions: &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
        ],
    },
    InferenceProfileGeography {
        prefix: "eu",
        label: "EU",
        regions: &["eu-central-1", "eu-north-1", "eu-west-1", "eu-west-3"],
    },
    InferenceProfileGeography {
        prefix: "us",
        label: "US",
        regions: &["us-east-1", "us-east-2", "us-west-2"],
    },
];

/// One base model Amazon Bedrock publishes a cross-region inference profile for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossRegionModel {
    /// `BedrockModelId` — the base model the profile routes invocations of, and the rest
    /// of the published `inferenceProfileId` behind its geography prefix. Every id here is
    /// a row of [`FOUNDATION_MODELS`], so the two catalogs agree about which models exist.
    pub model_id: &'static str,
    /// How the published `inferenceProfileName` spells the model, after the geography's
    /// own label.
    pub label: &'static str,
}

/// The base models this emulator's Amazon Bedrock publishes a cross-region profile for, in
/// `model_id` order.
///
/// The models AWS lists as supported for cross-region inference on "Supported Regions and
/// models for inference profiles", restricted to the ids [`FOUNDATION_MODELS`] carries so
/// no published profile routes to a model this control plane does not offer. A SUBSET of
/// what AWS offers, spanning two providers.
pub const CROSS_REGION_MODELS: &[CrossRegionModel] = &[
    CrossRegionModel {
        model_id: "amazon.nova-lite-v1:0",
        label: "Nova Lite",
    },
    CrossRegionModel {
        model_id: "amazon.nova-micro-v1:0",
        label: "Nova Micro",
    },
    CrossRegionModel {
        model_id: "amazon.nova-pro-v1:0",
        label: "Nova Pro",
    },
    CrossRegionModel {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        label: "Anthropic Claude 3 Haiku",
    },
    CrossRegionModel {
        model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
        label: "Anthropic Claude 3 Sonnet",
    },
];

/// The ARN of a system-defined inference profile in `region`:
/// `arn:aws:bedrock:<region>:<account>:inference-profile/<id>`.
///
/// `inference-profile` is the resource type `InferenceProfileArn`'s pattern admits beside
/// `application-inference-profile` (see [`application_inference_profile_arn`]), and it is
/// the one the profiles Amazon Bedrock publishes carry.
#[must_use]
pub fn system_defined_inference_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("inference-profile/{id}"))
}

/// Every system-defined inference profile Amazon Bedrock publishes in `region`, as the
/// `InferenceProfileSummary` a listing carries, in `inferenceProfileId` order.
///
/// A profile is published in the regions of its own geography, so a region belonging to no
/// geography this catalog carries has none. Each one reports one model per region of that
/// geography — the base model it routes invocations to there — and no `createdAt` or
/// `updatedAt`: those two members are optional, and nothing publishes an instant for a
/// profile no caller created.
#[must_use]
pub fn system_defined_inference_profiles(region: &str) -> Vec<InferenceProfileSummary> {
    let Some(geography) = INFERENCE_PROFILE_GEOGRAPHIES
        .iter()
        .find(|geography| geography.regions.contains(&region))
    else {
        return Vec::new();
    };
    CROSS_REGION_MODELS
        .iter()
        .map(|model| {
            let id = format!("{}.{}", geography.prefix, model.model_id);
            InferenceProfileSummary {
                inference_profile_name: format!("{} {}", geography.label, model.label),
                description: None,
                created_at: None,
                updated_at: None,
                inference_profile_arn: system_defined_inference_profile_arn(region, &id),
                models: geography
                    .regions
                    .iter()
                    .map(|routed| InferenceProfileModel {
                        model_arn: Some(foundation_model_arn(routed, model.model_id)),
                    })
                    .collect(),
                inference_profile_id: id,
                status: INFERENCE_PROFILE_ACTIVE.to_string(),
                r#type: INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED.to_string(),
            }
        })
        .collect()
}

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
