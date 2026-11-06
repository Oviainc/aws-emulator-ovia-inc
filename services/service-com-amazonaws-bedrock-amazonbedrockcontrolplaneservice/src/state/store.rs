// SPDX-License-Identifier: CC0-1.0

//! The per-service STORE: the state struct a handler reaches through
//! [`crate::state::service_state`], and the tag writes every tagging operation makes
//! through it.

use super::*;

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
pub(super) fn arn_region(arn: &str) -> Option<&str> {
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
