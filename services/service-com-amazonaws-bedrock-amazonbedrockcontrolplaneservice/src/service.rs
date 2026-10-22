// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrock-amazonbedrockcontrolplaneservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! # This service's own ARN check
//!
//! For the resources in [`crate::state::SELF_VALIDATED_RESOURCE_TYPES`], Amazon Bedrock
//! reads an ARN a caller submits ITSELF, ahead of the constraints the model declares for
//! the member carrying it, and answers one of its OWN two short sentences for an ARN it
//! will not accept — never the `N validation error(s) detected: Value '…' at '…' failed
//! to satisfy constraint: Member must satisfy regular expression pattern: …` envelope the
//! framework words a constraint violation in. An ARN of any OTHER resource — and any
//! other refusal — keeps the framework's wording, which is what the corpus records for
//! them. Both sentences and the catalog live in [`crate::state`]; this file is where the
//! two layers that produce them sit:
//!
//! * [`ArnChecked`], the wrapper every modelled operation is served behind
//!   (`override_operation`), refuses an ARN of this service's own namespace that names
//!   ANOTHER region before the modelled walk runs. It is the layer that can: the walk's
//!   refusal reaches `override_refusal` with the violations ALONE — no request — so the
//!   region the request addresses is not knowable there.
//! * `override_refusal` words what is left: an ARN of this service's namespace that is
//!   in-region but is not a well-formed ARN of the resource type it names.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, PatternTrait};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};

/// The error code this service answers a refused ARN with: the modelled
/// `ValidationException`, which every operation the corpus records this refusal for
/// declares at `@httpError(400)` (`smithy_query(query="operation",…)`). It is also the
/// framework's own default code for a refusal, restated here because `override_refusal`
/// and the refusal channel replace the code and the message TOGETHER.
///
/// `CreateModelCopyJob` is the one operation that does NOT list it, yet the corpus
/// records Amazon Bedrock answering 400 `ValidationException` there too, so the code is
/// the same for every member: it is reported through the framework's refusal channel
/// rather than through an operation's error enum, which is what lets that one answer it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The input members whose ARN names a resource in ANOTHER region BY DESIGN, so
/// [`ArnChecked`]'s region check does not apply to them, keyed by the operation that
/// declares the member and the member's own path.
///
/// One member today. `CreateModelCopyJob` "Copies a model to another region so that it
/// can be used there" (`smithy_query(query="doc",shape="CreateModelCopyJob")`) and its
/// input declares no target-region member
/// (`smithy_query(query="operation",operation="CreateModelCopyJob")`: `sourceModelArn`,
/// `targetModelName`, `modelKmsKeyId`, `targetModelTags`, `clientRequestToken`), so the
/// region the request addresses IS the target and the source model it names lives in
/// another one. Refusing that ARN for naming another region would refuse the whole
/// operation.
const CROSS_REGION_ARN_INPUTS: &[(&str, &str)] = &[("CreateModelCopyJob", "sourceModelArn")];

/// The `com-amazonaws-bedrock-amazonbedrockcontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ArnChecked`] per modelled operation, keyed by the routing key the model
    /// names that operation by ([`Operation::NAME`], the same key the generated routing
    /// table uses). Built once here, never per request; each entry is zero-sized, so
    /// this is a table of names rather than a second set of operations.
    arn_checked: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            arn_checked: arn_checked_table::<Eng>(),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `bedrock` — a signing name it SHARES with every other
        // Bedrock-family service (`bedrock-runtime`, `bedrock-agent`,
        // `bedrock-agent-runtime`, `bedrock-data-automation` and
        // `bedrock-data-automation-runtime` all carry `aws.auth#sigv4` name `bedrock`
        // while serving their own endpoint prefix). The router resolves a request's
        // routing key from the SigV4 credential scope first, so all six see the key
        // `bedrock`: claiming on the signing name ALONE makes each of them a claimant for
        // every Bedrock request, and the router — which refuses to pick one — answers
        // `500 AmbiguousService`, a code no AWS model carries, for ALL of it, leaving the
        // whole family unreachable.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the one
        // rule `service_common::bedrock` states for the whole family (which narrows
        // `service_common::claims_request` with the family's shared-segment split): the
        // signing name AND a first URI path segment this `restJson1` model declares
        // (`guardrails`, `foundation-models`, `logging`, `custom-models`,
        // `model-customization-jobs`, …), matched by EXACT segment equality. `None` for
        // the RPC argument: every operation of this model is bound to a URI, so it names
        // no service out of band.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve every modelled operation through its [`ArnChecked`] wrapper, so an ARN
        // of this service's own namespace naming another region is refused with this
        // service's own sentence BEFORE the modelled constraint walk words a refusal of
        // its own. The wrapper delegates every other request to the generated operation
        // in the routing table, which stays the only thing that serves it.
        //
        // Keyed on the routing key the model names each operation by, so a request that
        // names no modelled operation resolves exactly as it did before.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.arn_checked.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Amazon Bedrock reports an ARN of its own namespace that it will not accept with
        // a short sentence of its own, not with the clause the framework writes for a
        // member outside its `@pattern`: the two are produced by different rules, and a
        // client that branches on the message shape takes the wrong branch when a
        // constraint envelope arrives where the service's own sentence belongs.
        //
        // The region half of that check is answered one layer earlier, by [`ArnChecked`]
        // (it needs the request). What reaches here is an ARN this service accepts the
        // REGION of and whose resource is not a well-formed one of the type the ARN
        // itself names, so the sentence is the format one.
        //
        // `find_map`, not "every violation": the ARN check runs ahead of the modelled
        // constraints, so it decides the answer even when the same request also breaks
        // another rule. Every OTHER refusal — a value outside a length, range or enum, a
        // member that is not an ARN at all, an absent required member — keeps the
        // framework's own wording, and so does an authentication refusal, which is not an
        // input-constraint failure at all.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: violations.iter().find_map(refused_arn_format)?,
            }),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has no wording of
            // its own for keeps the framework's.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

/// This service's own format sentence for `violation`, when the value it refused is an
/// ARN of a resource this service validates itself
/// ([`crate::state::SELF_VALIDATED_RESOURCE_TYPES`]) submitted to a member the MODEL
/// declares an ARN-admitting `@pattern` for.
///
/// Only a `@pattern` violation: a value outside a LENGTH bound is refused for its size
/// rather than for its ARN form, and an absent member carries no value to read an ARN
/// off at all. The declared expression comes with the violation, which is how this seam
/// — handed the violations alone — applies the same "the member admits an ARN" test
/// [`admits_an_arn`] makes against the input schema: a caller who puts an ARN into a
/// member that is not one (a name, a description, a tag value) breaks that member's own
/// rule, not an ARN rule, and the corpus records the constraint envelope for it.
fn refused_arn_format(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(declared) = &violation.kind else {
        return None;
    };
    if !pattern_admits_an_arn(declared) {
        return None;
    }
    crate::state::invalid_arn_format_message(violation.value.as_deref()?)
}

/// One modelled operation, served behind this service's own ARN check.
///
/// Zero-sized: the wrapper holds no engine and no operation. It carries the operation
/// TYPE, which is all it needs — the input shape to read the request into and the input
/// `Schema` to read each member's declared form off — and delegates every request it does
/// not refuse to the one operation instance the generated routing table built from the
/// shared engine. There is therefore no second instance of any operation and no second
/// store.
struct ArnChecked<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ArnChecked<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ArnChecked<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let region = crate::state::request_region(ctx.parts());
            // The protocol is read, used and dropped inside this one statement, and the
            // operation to delegate to is resolved into a value in the next one:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across the
            // await below would make this future non-`Send`.
            let refused = cross_region_arn::<Op>(svc.definition().protocol(), req, &region);
            if let Some((path, value)) = refused {
                return rejection::<Op>(svc, path, value);
            }
            let delegate = svc.definition().routing_table().get(Op::NAME);
            match delegate {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed (or not) as
        // the MODEL says for the operation it names, so authentication is decided by the
        // modelled operation this wrapper stands in front of.
        None
    }
}

/// The first ARN-bearing member of `req` whose value is an ARN of a resource this service
/// validates itself ([`crate::state::SELF_VALIDATED_RESOURCE_TYPES`]) naming a region
/// other than `region`, as its member path and the ARN it carried.
///
/// Read off the operation's own input: the request is deserialized against `Op`'s input
/// schema (so a member is placed by the model, not by where it happens to sit in the
/// wire form) and every string the built input carries is collected by
/// [`service_common::validation::submitted_leaves`], the one walk in the tree that spells
/// a member path the way the framework's constraint walk does.
///
/// Only a member the MODEL declares an ARN-admitting `@pattern` for is read, so a
/// description or a tag value that happens to contain an ARN is not one of these; and
/// only a TOP-LEVEL member, because a member schema carries its target's members
/// nowhere, so a nested member's declared form is not reachable from the input schema. No
/// ARN-bearing member of the operations this rule is recorded for is nested.
///
/// `None` when the request carries no such ARN — including a body no codec can read,
/// which is a failure the framework's own walk words.
///
/// KNOWN BOUND: the leaf walk stops at the budget its module states
/// (`MAX_RECORDED_LEAVES` = 256 leaves, `MAX_RECORDED_BYTES` = 64 KiB), so a request
/// whose earlier strings exhaust it — a 200-entry tag list ahead of the ARN member, say —
/// can leave a later top-level ARN uninspected. The miss is FAIL-SAFE in one direction
/// only: such a request falls back to the framework's constraint envelope (the divergence
/// this check removes) rather than being refused for an ARN it carries, so no request is
/// refused wrongly, but a very large one may still be worded wrongly. Closing it needs
/// the shared walk to offer an unbounded form (or the framework to hand a built input to
/// an operation), which is a change to shared code this issue does not own.
fn cross_region_arn<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    region: &str,
) -> Option<(String, String)> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, Op::INPUT_SCHEMA) else {
        return None;
    };
    let read = <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    let input = read.ok()?;
    service_common::validation::submitted_leaves(&input, Op::INPUT_SCHEMA)
        .into_iter()
        .find(|(member_path, value)| {
            admits_an_arn(Op::INPUT_SCHEMA, member_path)
                && !CROSS_REGION_ARN_INPUTS.contains(&(Op::NAME, member_path.as_str()))
                && crate::state::arn_names_another_region(value, region)
        })
}

/// Whether `input_schema` declares the top-level member at `member_path` with a
/// `@pattern` that admits an ARN.
///
/// Read off the member's own `@pattern` trait, so the MODEL decides which members carry
/// an ARN ([`pattern_admits_an_arn`]).
fn admits_an_arn(input_schema: &Schema, member_path: &str) -> bool {
    declared_pattern(input_schema, member_path).is_some_and(pattern_admits_an_arn)
}

/// Whether a declared `@pattern` admits an ARN.
///
/// Every ARN-valued member of this service declares an anchored expression over `arn:…`
/// — on its own (`ModelCopyJobArn`, `CustomModelArn`) or as one alternative beside a bare
/// name (`ProvisionedModelId`, `ModelInvocationJobIdentifier`), which is why ADMITTING
/// one is the test rather than requiring one. A member whose expression mentions no ARN
/// at all (a name, a description, a tag value) carries no ARN for this service to read,
/// whatever a caller puts in it.
fn pattern_admits_an_arn(pattern: &str) -> bool {
    pattern.contains("arn:")
}

/// The `@pattern` expression `input_schema` declares for its top-level member
/// `member_path`, if it declares one.
fn declared_pattern(input_schema: &Schema, member_path: &str) -> Option<&'static str> {
    let member = input_schema.members().iter().find(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_path)
    })?;
    member
        .traits()?
        .get(&PatternTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<PatternTrait>()
        .map(|pattern| pattern.regex)
}

/// This service's answer to an ARN it refused for naming another region: its own
/// envelope and the status the framework's refusal channel gives a refused input (400),
/// with [`VALIDATION_EXCEPTION`] and
/// [`crate::state::ARN_INVALID_FOR_SERVICE_REGION`] replacing the code and the message
/// together — a client branches on the code, so one without the other is worse than
/// neither.
///
/// The violation carried on the channel names the member and the form the model declares
/// for it, because that is what the channel's structure is; its clause never reaches the
/// wire, since the chosen code and message replace it.
fn rejection<Op: Operation>(
    svc: &dyn Service,
    member_path: String,
    value: String,
) -> aws_smithy_runtime_api::http::Response {
    let declared = declared_pattern(Op::INPUT_SCHEMA, &member_path).unwrap_or_default();
    svc.definition().refusal_response(
        ProtocolError::Invalid(vec![Violation {
            path: member_path,
            kind: ViolationKind::Pattern(declared.to_string()),
            value: Some(value),
        }]),
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: crate::state::ARN_INVALID_FOR_SERVICE_REGION.to_string(),
        }),
    )
}

/// The [`ArnChecked`] wrapper for EVERY operation this service serves.
///
/// Carrying an ARN is a property of an operation's INPUT SHAPE, and this service's ARN
/// check is the same for every member that carries one, so the wrapper goes in front of
/// all of them — implemented or still scaffolded. A stub answers 501 for a request whose
/// ARNs this service accepts and the ARN refusal for one it does not, which is the order
/// the real service evaluates them in; leaving an operation out would answer the
/// framework's generic constraint envelope there, the very divergence this removes.
fn arn_checked_table<Eng: Send + Sync + 'static>() -> HashMap<&'static str, Box<dyn ErasedOperation>>
{
    let mut wrappers: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                wrappers.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(ArnChecked::<crate::ops::$module::$operation<Eng>>::new())
                        as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    register!(
        batch_delete_advanced_prompt_optimization_job::BatchDeleteAdvancedPromptOptimizationJob,
        batch_delete_evaluation_job::BatchDeleteEvaluationJob,
        cancel_automated_reasoning_policy_build_workflow::CancelAutomatedReasoningPolicyBuildWorkflow,
        create_advanced_prompt_optimization_job::CreateAdvancedPromptOptimizationJob,
        create_automated_reasoning_policy::CreateAutomatedReasoningPolicy,
        create_automated_reasoning_policy_test_case::CreateAutomatedReasoningPolicyTestCase,
        create_automated_reasoning_policy_version::CreateAutomatedReasoningPolicyVersion,
        create_custom_model::CreateCustomModel,
        create_custom_model_deployment::CreateCustomModelDeployment,
        create_evaluation_job::CreateEvaluationJob,
        create_foundation_model_agreement::CreateFoundationModelAgreement,
        create_guardrail::CreateGuardrail,
        create_guardrail_version::CreateGuardrailVersion,
        create_inference_profile::CreateInferenceProfile,
        create_marketplace_model_endpoint::CreateMarketplaceModelEndpoint,
        create_model_copy_job::CreateModelCopyJob,
        create_model_customization_job::CreateModelCustomizationJob,
        create_model_import_job::CreateModelImportJob,
        create_model_invocation_job::CreateModelInvocationJob,
        create_prompt_router::CreatePromptRouter,
        create_provisioned_model_throughput::CreateProvisionedModelThroughput,
        delete_automated_reasoning_policy::DeleteAutomatedReasoningPolicy,
        delete_automated_reasoning_policy_build_workflow::DeleteAutomatedReasoningPolicyBuildWorkflow,
        delete_automated_reasoning_policy_test_case::DeleteAutomatedReasoningPolicyTestCase,
        delete_custom_model::DeleteCustomModel,
        delete_custom_model_deployment::DeleteCustomModelDeployment,
        delete_enforced_guardrail_configuration::DeleteEnforcedGuardrailConfiguration,
        delete_foundation_model_agreement::DeleteFoundationModelAgreement,
        delete_guardrail::DeleteGuardrail,
        delete_imported_model::DeleteImportedModel,
        delete_inference_profile::DeleteInferenceProfile,
        delete_marketplace_model_endpoint::DeleteMarketplaceModelEndpoint,
        delete_model_invocation_logging_configuration::DeleteModelInvocationLoggingConfiguration,
        delete_prompt_router::DeletePromptRouter,
        delete_provisioned_model_throughput::DeleteProvisionedModelThroughput,
        delete_resource_policy::DeleteResourcePolicy,
        deregister_marketplace_model_endpoint::DeregisterMarketplaceModelEndpoint,
        export_automated_reasoning_policy_version::ExportAutomatedReasoningPolicyVersion,
        get_account_data_retention::GetAccountDataRetention,
        get_advanced_prompt_optimization_job::GetAdvancedPromptOptimizationJob,
        get_automated_reasoning_policy::GetAutomatedReasoningPolicy,
        get_automated_reasoning_policy_annotations::GetAutomatedReasoningPolicyAnnotations,
        get_automated_reasoning_policy_build_workflow::GetAutomatedReasoningPolicyBuildWorkflow,
        get_automated_reasoning_policy_build_workflow_result_assets::GetAutomatedReasoningPolicyBuildWorkflowResultAssets,
        get_automated_reasoning_policy_next_scenario::GetAutomatedReasoningPolicyNextScenario,
        get_automated_reasoning_policy_test_case::GetAutomatedReasoningPolicyTestCase,
        get_automated_reasoning_policy_test_result::GetAutomatedReasoningPolicyTestResult,
        get_custom_model::GetCustomModel,
        get_custom_model_deployment::GetCustomModelDeployment,
        get_evaluation_job::GetEvaluationJob,
        get_foundation_model::GetFoundationModel,
        get_foundation_model_availability::GetFoundationModelAvailability,
        get_guardrail::GetGuardrail,
        get_imported_model::GetImportedModel,
        get_inference_profile::GetInferenceProfile,
        get_marketplace_model_endpoint::GetMarketplaceModelEndpoint,
        get_model_copy_job::GetModelCopyJob,
        get_model_customization_job::GetModelCustomizationJob,
        get_model_import_job::GetModelImportJob,
        get_model_invocation_job::GetModelInvocationJob,
        get_model_invocation_logging_configuration::GetModelInvocationLoggingConfiguration,
        get_prompt_router::GetPromptRouter,
        get_provisioned_model_throughput::GetProvisionedModelThroughput,
        get_resource_policy::GetResourcePolicy,
        get_use_case_for_model_access::GetUseCaseForModelAccess,
        list_advanced_prompt_optimization_jobs::ListAdvancedPromptOptimizationJobs,
        list_automated_reasoning_policies::ListAutomatedReasoningPolicies,
        list_automated_reasoning_policy_build_workflows::ListAutomatedReasoningPolicyBuildWorkflows,
        list_automated_reasoning_policy_test_cases::ListAutomatedReasoningPolicyTestCases,
        list_automated_reasoning_policy_test_results::ListAutomatedReasoningPolicyTestResults,
        list_custom_model_deployments::ListCustomModelDeployments,
        list_custom_models::ListCustomModels,
        list_enforced_guardrails_configuration::ListEnforcedGuardrailsConfiguration,
        list_evaluation_jobs::ListEvaluationJobs,
        list_foundation_model_agreement_offers::ListFoundationModelAgreementOffers,
        list_foundation_models::ListFoundationModels,
        list_guardrails::ListGuardrails,
        list_imported_models::ListImportedModels,
        list_inference_profiles::ListInferenceProfiles,
        list_marketplace_model_endpoints::ListMarketplaceModelEndpoints,
        list_model_copy_jobs::ListModelCopyJobs,
        list_model_customization_jobs::ListModelCustomizationJobs,
        list_model_import_jobs::ListModelImportJobs,
        list_model_invocation_jobs::ListModelInvocationJobs,
        list_prompt_routers::ListPromptRouters,
        list_provisioned_model_throughputs::ListProvisionedModelThroughputs,
        list_tags_for_resource::ListTagsForResource,
        put_account_data_retention::PutAccountDataRetention,
        put_enforced_guardrail_configuration::PutEnforcedGuardrailConfiguration,
        put_model_invocation_logging_configuration::PutModelInvocationLoggingConfiguration,
        put_resource_policy::PutResourcePolicy,
        put_use_case_for_model_access::PutUseCaseForModelAccess,
        register_marketplace_model_endpoint::RegisterMarketplaceModelEndpoint,
        start_automated_reasoning_policy_build_workflow::StartAutomatedReasoningPolicyBuildWorkflow,
        start_automated_reasoning_policy_test_workflow::StartAutomatedReasoningPolicyTestWorkflow,
        stop_advanced_prompt_optimization_job::StopAdvancedPromptOptimizationJob,
        stop_evaluation_job::StopEvaluationJob,
        stop_model_customization_job::StopModelCustomizationJob,
        stop_model_invocation_job::StopModelInvocationJob,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_automated_reasoning_policy::UpdateAutomatedReasoningPolicy,
        update_automated_reasoning_policy_annotations::UpdateAutomatedReasoningPolicyAnnotations,
        update_automated_reasoning_policy_test_case::UpdateAutomatedReasoningPolicyTestCase,
        update_custom_model_deployment::UpdateCustomModelDeployment,
        update_guardrail::UpdateGuardrail,
        update_marketplace_model_endpoint::UpdateMarketplaceModelEndpoint,
        update_provisioned_model_throughput::UpdateProvisionedModelThroughput,
    );
    wrappers
}
