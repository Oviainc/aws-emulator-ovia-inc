// SPDX-License-Identifier: CC0-1.0

//! The QUOTAS, status values, settle spans and refusal sentences this service answers
//! with, stated once each so two operations cannot spell the same limit or the same
//! refusal two ways.

use super::*;

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
            resource_type_not_found_message(resource_type)
        }
        _ => "Resource not found".to_string(),
    }
}

/// What this service answers for an ARN of one of its own resource types that names no such
/// resource: the type as the noun phrase that opens the sentence, and nothing of the request
/// in it.
///
/// RECORDED behaviour, stated once here because the sentence belongs to the RESOURCE TYPE
/// rather than to the operation that reported it missing: a resource of one type reports
/// itself absent the same way whichever member carried its ARN. The sentence carries no
/// full stop, which is how the corpus records it; the one resource whose recorded sentence
/// ends in one keeps its own constant ([`CUSTOM_MODEL_NOT_FOUND_MESSAGE`]) rather than being
/// bent to this rendering, because a message is recorded wording and not a format to
/// normalise.
#[must_use]
pub fn resource_type_not_found_message(resource_type: &str) -> String {
    format!("{} not found", resource_type_noun(resource_type))
}

/// A resource type as the noun phrase that OPENS a sentence about it: the type spelled as
/// words ([`spelled_resource_type`]) with its first word capitalised
/// (`custom-model-deployment` is "Custom model deployment").
pub(super) fn resource_type_noun(resource_type: &str) -> String {
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
pub(super) fn spelled_resource_type(resource_type: &str) -> String {
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

/// `EvaluationJobStatus` — the job ended without producing its results: the terminal status
/// a job whose configured run could not be performed reaches
/// ([`crate::state::evaluation_run`]), reported with the sentences it failed on in
/// `failureMessages`. It is also one of the statuses a batch delete accepts.
pub const EVALUATION_JOB_STATUS_FAILED: &str = "Failed";

/// The most `failureMessages` an evaluation job reports.
///
/// The MODELLED ceiling: `GetEvaluationJobResponse$failureMessages` targets `ErrorMessages`,
/// which is `@length(max: 20)`
/// (`smithy_query(query="operation",operation="GetEvaluationJob")`), so a run that failed on
/// more than that reports the first of them rather than a list the response cannot carry.
pub const MAX_EVALUATION_FAILURE_MESSAGES: usize = 20;

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

/// What Amazon Bedrock answers when an `EvaluationJobIdentifier` names no evaluation job
/// this account holds: the modelled `ResourceNotFoundException` (`@httpError(404)`).
///
/// RECORDED behaviour, with the identifier the request carried as its one variable part:
/// Amazon Bedrock reports that the job does not exist and tells the caller what to do about
/// it. The operations that resolve such an identifier answer with ONE sentence — the read
/// and the stop address the same resource by the same identifier, so a job neither of them
/// finds is reported the same way.
#[must_use]
pub fn evaluation_job_not_found(identifier: &str) -> String {
    format!("Evaluation job {identifier} does not exist. Please verify the job arn and retry.")
}

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

/// How a value submitted to a member that IDENTIFIES a model — a `ModelIdentifier`, an
/// `ImportedModelIdentifier`, a `baseModelIdentifier`, a `modelId` — and which this
/// service resolved to NO model is refused.
///
/// Such a member admits several spellings of one model (the ARN of a resource this service
/// holds, a bare foundation model id, a bare model name), and an unresolvable value is not
/// the same failure in each: WHICH it is comes off the spelling the caller used, never off
/// what the store happens to hold.
///
/// * an ARN of one of the resource types the operation RESOLVES that member against names a
///   resource to report missing — `ResourceNotFoundException` (404), which is what that
///   error documents itself as ("The specified resource Amazon Resource Name (ARN) was not
///   found. Check the Amazon Resource Name (ARN) and try your request again") — carrying
///   that resource type's own not-found sentence;
/// * every other spelling the member admits is a value this service cannot read as the
///   model the operation addresses, which is an INPUT fault: `ValidationException` (400)
///   carrying the fixed [`INVALID_MODEL_IDENTIFIER_MESSAGE`].
///
/// Keeping them apart is what lets a caller act on the answer: 400 says "fix the request",
/// where 404 is routinely retried as eventual consistency. It is the same split the
/// published catalogue makes for a `modelIdentifier` ([`unoffered_foundation_model`]) and
/// the inference plane makes for the same member, read here for every member that resolves
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedModel {
    /// An ARN of one of the resource types the operation resolves the identifier against,
    /// naming no such resource — the type it named.
    MissingResource(&'static str),
    /// Any other spelling the member admits: a bare foundation model id, a bare model name,
    /// an ARN of a resource type this operation does not resolve the member against.
    UnreadableIdentifier,
}

impl UnresolvedModel {
    /// Whether this reading reports a missing RESOURCE (404) rather than un-usable input
    /// (400) — the one question an operation asks to pick the error its own model declares
    /// for the class.
    #[must_use]
    pub fn missing_resource(self) -> bool {
        matches!(self, Self::MissingResource(_))
    }

    /// The sentence this refusal carries: the not-found sentence of the resource type the
    /// ARN named, or the fixed one for an identifier this service cannot read.
    ///
    /// Fixed text for every reading whose fault is the CLASS of the identifier rather than
    /// the value in it; the missing `foundation-model` ARN echoes the ARN the request
    /// addressed, because that is the resource it reports absent and that is how the
    /// catalogue's own read is recorded reporting it.
    #[must_use]
    pub fn message(self, identifier: &str) -> String {
        match self {
            Self::MissingResource(CUSTOM_MODEL_RESOURCE_TYPE) => {
                CUSTOM_MODEL_NOT_FOUND_MESSAGE.to_string()
            }
            Self::MissingResource(FOUNDATION_MODEL_RESOURCE_TYPE) => {
                foundation_model_not_found(identifier)
            }
            Self::MissingResource(resource_type) => resource_type_not_found_message(resource_type),
            Self::UnreadableIdentifier => INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
        }
    }
}

/// How `identifier` — a value submitted to a model-identifying member that this service
/// resolved to no model — is refused, where `resolved_against` are the ARN resource types
/// the operation looked it up in.
///
/// Stated once because the reading belongs to the IDENTIFIER rather than to one operation:
/// every operation that resolves a model identifier refuses one it cannot resolve the same
/// way, and `resolved_against` is the only thing that differs between them — the families
/// that operation asks, which is what decides whether an ARN spelling names a resource
/// there is anything to report missing. The spelling is read through this crate's one ARN
/// parse, so no two readings of "is this an ARN of ours" disagree; a value that is no ARN
/// of this service's namespace names no resource of it at all.
#[must_use]
pub fn unresolved_model(identifier: &str, resolved_against: &[&'static str]) -> UnresolvedModel {
    let Some(arn) = ::service_common::arn::parse_of(identifier, ARN_NAMESPACE) else {
        return UnresolvedModel::UnreadableIdentifier;
    };
    let named = arn.resource_type();
    resolved_against
        .iter()
        .find(|resource_type| **resource_type == named)
        .map_or(UnresolvedModel::UnreadableIdentifier, |resource_type| {
            UnresolvedModel::MissingResource(resource_type)
        })
}

/// The ARN resource types the imported model family resolves an `ImportedModelIdentifier`
/// against: the model's own ARN, beside the bare model NAME the same member admits
/// (`smithy_query(query="operation",operation="GetImportedModel")`).
pub const IMPORTED_MODEL_RESOURCES: &[&str] = &[IMPORTED_MODEL_RESOURCE_TYPE];

/// The ARN resource types a base-model member is resolved against: the published
/// catalogue's `foundation-model` ARNs and the `custom-model` ARNs of this account, which
/// are the two ARN forms `BaseModelIdentifier` and `ModelIdentifier` admit
/// (`smithy_query(query="shape",shape="BaseModelIdentifier,ModelIdentifier")`).
pub const BASE_MODEL_RESOURCES: &[&str] =
    &[FOUNDATION_MODEL_RESOURCE_TYPE, CUSTOM_MODEL_RESOURCE_TYPE];

/// The ARN resource types a custom-model member is resolved against: only the model's own
/// ARN. `ModelIdentifier` admits a `foundation-model` ARN too, but an operation of the
/// custom model resource resolves the member against the CUSTOM models alone, so such a
/// value names no resource it could report missing.
pub const CUSTOM_MODEL_RESOURCES: &[&str] = &[CUSTOM_MODEL_RESOURCE_TYPE];

/// The ARN resource types a batch inference `modelId` is resolved against: the two base
/// model forms, and the inference profiles the same member admits
/// (`smithy_query(query="shape",shape="ModelId")`).
pub const INVOCATION_MODEL_RESOURCES: &[&str] = &[
    FOUNDATION_MODEL_RESOURCE_TYPE,
    CUSTOM_MODEL_RESOURCE_TYPE,
    INFERENCE_PROFILE_RESOURCE_TYPE,
    APPLICATION_INFERENCE_PROFILE_RESOURCE_TYPE,
];

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

/// Whether `base_model_identifier` is spelled the way Amazon Bedrock identifies the base
/// model a CUSTOMIZATION job runs on, which is read ahead of everything else the request
/// names — the `iam:PassRole` step for `roleArn` included.
///
/// A customization runs on a TRAINING VARIANT of a published model rather than on the model
/// a caller invokes on demand, and Amazon Bedrock publishes that variant under the model id
/// with its version qualifiers (`<provider>.<model>:<version>:<context length>`,
/// `ModelCustomizationJobArn` admits `([:][a-z0-9-]{1,63}){0,2}` after the model name for
/// exactly that reason). So two spellings identify one:
///
/// * an ARN of a model resource of this service's namespace — the published model's
///   `foundation-model` ARN, or the `custom-model` ARN of a model this account customized
///   already, which names the model RESOURCE and carries its own qualifiers;
/// * a bare model id carrying at least one of those qualifiers.
///
/// A bare `<provider>.<model>` id with no qualifier at all names the on-demand model and no
/// customizable variant of it, so it identifies nothing this operation can customize and is
/// refused with [`INVALID_MODEL_IDENTIFIER_MESSAGE`] — RECORDED behaviour, and recorded for
/// the two spellings in both directions: the corpus records that refusal for an unqualified
/// bare id, and records an ARN-spelled identifier of a model this service offers no longer
/// reaching the pass-role decision behind this check instead.
///
/// This is a question about the identifier's FORM, so it reads no state and resolves
/// nothing: whether the variant it names exists is the catalogue's answer, taken behind the
/// pass-role step.
#[must_use]
pub fn names_a_customizable_base_model(base_model_identifier: &str) -> bool {
    if ::service_common::arn::parse_of(base_model_identifier, ARN_NAMESPACE).is_some() {
        return true;
    }
    // The bare form: a `<provider>.<model>` id followed by at least one `:<qualifier>`
    // segment, every qualifier non-empty — the shape `ModelCustomizationJobArn` spells
    // after the model name.
    let Some((model, qualifiers)) = base_model_identifier.split_once(':') else {
        return false;
    };
    model.contains('.')
        && !qualifiers.is_empty()
        && qualifiers.split(':').all(|qualifier| !qualifier.is_empty())
}

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
pub(super) const ID_HALF_WIDTH: usize = 6;
