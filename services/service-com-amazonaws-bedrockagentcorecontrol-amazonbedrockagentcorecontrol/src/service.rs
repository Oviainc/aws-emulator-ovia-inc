// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol` service: how it claims requests and how it words the
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
//!   correctly. This service DOES use it, not to re-route but to serve each
//!   modelled operation wrapped in [`StatedPerOperation`]; see below.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. It states one through [`validation`], the same module
//!   [`StatedPerOperation`] states one through.
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
//! ## This service's edited seam: how a broken constraint is stated
//!
//! [`validation`] owns the sentence (its module docs state the rules); this file owns the
//! seam that feeds it. Three parts of that sentence the framework cannot produce on its
//! own, because [`Service::override_refusal`] is handed the violations ALONE — no
//! operation, no request:
//!
//! * WHICH BACKEND answered. This endpoint fronts several AgentCore services, one per
//!   resource family, and the recorded refusals of the `/memories`,
//!   `/configuration-bundles` and `/identities` families quote no value while every other
//!   family quotes the one the request carried — for violations that are otherwise
//!   identical (the same member path, the same `@pattern`). The family is a property of the
//!   OPERATION, read off the `@http` URI its input schema declares.
//! * the ELEMENT CONSTRAINT SET a clause states at a collection member, which is read off
//!   that operation's input schema rather than off the refusal (a refusal carries only the
//!   constraints that failed).
//! * the VALUE a clause echoes: the framework's walk stores it ELIDED (a 256-byte prefix
//!   plus `... [<n> bytes]`, a marker AWS never emits), and the value a clause states at a
//!   LIST member is the whole list, which no violation carries.
//!
//! [`StatedPerOperation`] is where all three come from: every modelled operation is served
//! wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//! violations against that operation's input schema — echoes restored — together with the
//! request's own lists, and [`validation`] turns them into the sentence. `override_refusal`
//! words a refusal that reaches the centralized walk anyway through that same module, so
//! the wording lives in ONE place.
//!
//! ## This service's other edited seam: the endpoint a gateway publishes
//!
//! A gateway's `gatewayUrl` is a host of its OWN
//! (`<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com`), not this service's
//! control-plane endpoint, and what a client drives on it is the Model Context Protocol
//! rather than a modelled operation. `can_handle` therefore also claims that host — no
//! routing key can name it, so without the claim the router answers `404 UnknownService`
//! for a URL this service handed out — and `override_operation` names [`mcp`]'s surface
//! for it, so the emulator verifies the credential the request carries before anything is
//! served. [`mcp`] documents what answers there.

pub mod mcp;
pub(crate) mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The host-style endpoint this service is addressed by:
/// `bedrock-agentcore-control.<region>.amazonaws.com`.
///
/// Source: `smithy_query(query="service_info", model_path="services/\
/// service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/\
/// smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`
/// — `awsService.endpointPrefix` is `bedrock-agentcore-control`, while `sigv4Name` is the
/// shorter `bedrock-agentcore`. The two differ, so the endpoint name is NOT derivable
/// from the generated identity and is stated here.
const ENDPOINT_PREFIX: &str = "bedrock-agentcore-control";

/// The `com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the entry constructed every operation from, kept so
    /// the pre-authorization seam below can read this service's store. A routing
    /// decision that depends on stored state has no handler to read it in.
    engine: Arc<Eng>,
    /// The denial that seam serves, built once (it holds nothing per-request)
    /// because `override_operation` hands back a borrow of `&self`.
    denial: CapacityProviderDenial,
    /// Serves the MCP wire surface a gateway publishes — the endpoint `CreateGateway`
    /// hands back in `gatewayUrl`. Held on the service because
    /// [`Service::override_operation`] hands the router a BORROW of one of its own
    /// operations, and it owns the shared engine so every answer is computed from the
    /// same store the control-plane handlers write.
    gateway_mcp: mcp::GatewayMcpSurface<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way the backend serving
    /// that operation words it. Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    stated: OperationRoutingTable,
}

/// Answers one request with this service's modelled permissions refusal: HTTP 403
/// `AccessDeniedException`, in the service's OWN protocol envelope.
///
/// An [`ErasedOperation`] rather than a modelled `Operation` because it answers a
/// request the framework's centralized walk would refuse FIRST: that walk validates
/// the model's input constraints before any handler runs, and this refusal is an
/// authorization decision, which AWS takes before it validates. The envelope is not
/// hand-rolled: it is rendered by the service's own model-derived renderer
/// (`ErasedServiceDefinition::framework_error_response`), the same one every
/// framework-level refusal of this service goes through, so the code, the status and
/// the `x-amzn-errortype` header are the protocol's and not this file's.
///
/// The WORDS are the service's, stated once in
/// [`crate::state::CAPACITY_PROVIDER_DENIAL`] and shared with the handler, so the seam
/// and the operation cannot tell a caller two different things.
struct CapacityProviderDenial;

impl ErasedOperation for CapacityProviderDenial {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let _ = ctx;
        Box::pin(async move {
            svc.definition().framework_error_response(
                ProtocolError::Unauthenticated {
                    code: crate::state::ACCESS_DENIED_SHAPE,
                    message: crate::state::CAPACITY_PROVIDER_DENIAL,
                },
                req,
            )
        })
    }
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut stated = OperationRoutingTable::new();
        // EVERY modelled operation (171 of them, one per file in `src/ops/`), because the
        // wording rule is the SERVICE's and not one operation's: which family an operation
        // belongs to, and what its collections' elements must satisfy, are read off that
        // operation's own input schema, so one code path words every refusal. An operation
        // whose input declares no constraint simply never has a violation to state, and a
        // still-scaffolded one is wrapped too — the framework refuses a bad request before
        // any handler runs, so its refusals are this service's words even while its success
        // path is a 501. An operation added later is added here as well, or it answers a
        // refusal in the framework's wording rather than this service's;
        // `tests::every_modelled_operation_is_served_with_this_services_refusal_wording`
        // fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            add_dataset_examples::AddDatasetExamples,
            batch_put_gateway_rate_limits::BatchPutGatewayRateLimits,
            create_agent_runtime::CreateAgentRuntime,
            create_agent_runtime_endpoint::CreateAgentRuntimeEndpoint,
            create_api_key_credential_provider::CreateApiKeyCredentialProvider,
            create_browser::CreateBrowser,
            create_browser_profile::CreateBrowserProfile,
            create_capacity_provider::CreateCapacityProvider,
            create_code_interpreter::CreateCodeInterpreter,
            create_configuration_bundle::CreateConfigurationBundle,
            create_consent_portal::CreateConsentPortal,
            create_dataset::CreateDataset,
            create_dataset_version::CreateDatasetVersion,
            create_evaluator::CreateEvaluator,
            create_gateway::CreateGateway,
            create_gateway_rate_limit::CreateGatewayRateLimit,
            create_gateway_rule::CreateGatewayRule,
            create_gateway_target::CreateGatewayTarget,
            create_harness::CreateHarness,
            create_harness_endpoint::CreateHarnessEndpoint,
            create_memory::CreateMemory,
            create_oauth2_credential_provider::CreateOauth2CredentialProvider,
            create_online_evaluation_config::CreateOnlineEvaluationConfig,
            create_payment_connector::CreatePaymentConnector,
            create_payment_credential_provider::CreatePaymentCredentialProvider,
            create_payment_manager::CreatePaymentManager,
            create_policy::CreatePolicy,
            create_policy_engine::CreatePolicyEngine,
            create_registry::CreateRegistry,
            create_registry_record::CreateRegistryRecord,
            create_workload_identity::CreateWorkloadIdentity,
            delete_agent_runtime::DeleteAgentRuntime,
            delete_agent_runtime_endpoint::DeleteAgentRuntimeEndpoint,
            delete_api_key_credential_provider::DeleteApiKeyCredentialProvider,
            delete_browser::DeleteBrowser,
            delete_browser_profile::DeleteBrowserProfile,
            delete_capacity_provider::DeleteCapacityProvider,
            delete_code_interpreter::DeleteCodeInterpreter,
            delete_configuration_bundle::DeleteConfigurationBundle,
            delete_consent_portal::DeleteConsentPortal,
            delete_dataset::DeleteDataset,
            delete_dataset_examples::DeleteDatasetExamples,
            delete_evaluator::DeleteEvaluator,
            delete_gateway::DeleteGateway,
            delete_gateway_rate_limit::DeleteGatewayRateLimit,
            delete_gateway_rule::DeleteGatewayRule,
            delete_gateway_target::DeleteGatewayTarget,
            delete_harness::DeleteHarness,
            delete_harness_endpoint::DeleteHarnessEndpoint,
            delete_memory::DeleteMemory,
            delete_oauth2_credential_provider::DeleteOauth2CredentialProvider,
            delete_online_evaluation_config::DeleteOnlineEvaluationConfig,
            delete_payment_connector::DeletePaymentConnector,
            delete_payment_credential_provider::DeletePaymentCredentialProvider,
            delete_payment_manager::DeletePaymentManager,
            delete_policy::DeletePolicy,
            delete_policy_engine::DeletePolicyEngine,
            delete_registry::DeleteRegistry,
            delete_registry_record::DeleteRegistryRecord,
            delete_resource_policy::DeleteResourcePolicy,
            delete_workload_identity::DeleteWorkloadIdentity,
            get_agent_runtime::GetAgentRuntime,
            get_agent_runtime_endpoint::GetAgentRuntimeEndpoint,
            get_api_key_credential_provider::GetApiKeyCredentialProvider,
            get_browser::GetBrowser,
            get_browser_profile::GetBrowserProfile,
            get_capacity_provider::GetCapacityProvider,
            get_code_interpreter::GetCodeInterpreter,
            get_configuration_bundle::GetConfigurationBundle,
            get_configuration_bundle_version::GetConfigurationBundleVersion,
            get_consent_portal::GetConsentPortal,
            get_dataset::GetDataset,
            get_evaluator::GetEvaluator,
            get_gateway::GetGateway,
            get_gateway_rate_limit::GetGatewayRateLimit,
            get_gateway_rule::GetGatewayRule,
            get_gateway_target::GetGatewayTarget,
            get_harness::GetHarness,
            get_harness_endpoint::GetHarnessEndpoint,
            get_memory::GetMemory,
            get_oauth2_credential_provider::GetOauth2CredentialProvider,
            get_online_evaluation_config::GetOnlineEvaluationConfig,
            get_payment_connector::GetPaymentConnector,
            get_payment_credential_provider::GetPaymentCredentialProvider,
            get_payment_manager::GetPaymentManager,
            get_policy::GetPolicy,
            get_policy_engine::GetPolicyEngine,
            get_policy_engine_summary::GetPolicyEngineSummary,
            get_policy_generation::GetPolicyGeneration,
            get_policy_generation_summary::GetPolicyGenerationSummary,
            get_policy_summary::GetPolicySummary,
            get_registry::GetRegistry,
            get_registry_record::GetRegistryRecord,
            get_resource_policy::GetResourcePolicy,
            get_token_vault::GetTokenVault,
            get_workload_identity::GetWorkloadIdentity,
            list_agent_runtime_endpoints::ListAgentRuntimeEndpoints,
            list_agent_runtime_versions::ListAgentRuntimeVersions,
            list_agent_runtime_versions_by_capacity_provider::ListAgentRuntimeVersionsByCapacityProvider,
            list_agent_runtimes::ListAgentRuntimes,
            list_api_key_credential_providers::ListApiKeyCredentialProviders,
            list_browser_profiles::ListBrowserProfiles,
            list_browsers::ListBrowsers,
            list_capacity_providers::ListCapacityProviders,
            list_code_interpreters::ListCodeInterpreters,
            list_configuration_bundle_versions::ListConfigurationBundleVersions,
            list_configuration_bundles::ListConfigurationBundles,
            list_consent_portals::ListConsentPortals,
            list_dataset_examples::ListDatasetExamples,
            list_dataset_versions::ListDatasetVersions,
            list_datasets::ListDatasets,
            list_evaluators::ListEvaluators,
            list_gateway_rate_limits::ListGatewayRateLimits,
            list_gateway_rules::ListGatewayRules,
            list_gateway_targets::ListGatewayTargets,
            list_gateways::ListGateways,
            list_harness_endpoints::ListHarnessEndpoints,
            list_harness_versions::ListHarnessVersions,
            list_harnesses::ListHarnesses,
            list_memories::ListMemories,
            list_oauth2_credential_providers::ListOauth2CredentialProviders,
            list_online_evaluation_configs::ListOnlineEvaluationConfigs,
            list_payment_connectors::ListPaymentConnectors,
            list_payment_credential_providers::ListPaymentCredentialProviders,
            list_payment_managers::ListPaymentManagers,
            list_policies::ListPolicies,
            list_policy_engine_summaries::ListPolicyEngineSummaries,
            list_policy_engines::ListPolicyEngines,
            list_policy_generation_assets::ListPolicyGenerationAssets,
            list_policy_generation_summaries::ListPolicyGenerationSummaries,
            list_policy_generations::ListPolicyGenerations,
            list_policy_summaries::ListPolicySummaries,
            list_registries::ListRegistries,
            list_registry_records::ListRegistryRecords,
            list_tags_for_resource::ListTagsForResource,
            list_workload_identities::ListWorkloadIdentities,
            put_resource_policy::PutResourcePolicy,
            rotate_payment_connector_credentials::RotatePaymentConnectorCredentials,
            set_token_vault_cmk::SetTokenVaultCMK,
            start_policy_generation::StartPolicyGeneration,
            submit_registry_record_for_approval::SubmitRegistryRecordForApproval,
            synchronize_gateway_targets::SynchronizeGatewayTargets,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_agent_runtime::UpdateAgentRuntime,
            update_agent_runtime_endpoint::UpdateAgentRuntimeEndpoint,
            update_api_key_credential_provider::UpdateApiKeyCredentialProvider,
            update_capacity_provider::UpdateCapacityProvider,
            update_configuration_bundle::UpdateConfigurationBundle,
            update_consent_portal::UpdateConsentPortal,
            update_dataset::UpdateDataset,
            update_dataset_examples::UpdateDatasetExamples,
            update_evaluator::UpdateEvaluator,
            update_gateway::UpdateGateway,
            update_gateway_rate_limit::UpdateGatewayRateLimit,
            update_gateway_rule::UpdateGatewayRule,
            update_gateway_target::UpdateGatewayTarget,
            update_harness::UpdateHarness,
            update_harness_endpoint::UpdateHarnessEndpoint,
            update_memory::UpdateMemory,
            update_oauth2_credential_provider::UpdateOauth2CredentialProvider,
            update_online_evaluation_config::UpdateOnlineEvaluationConfig,
            update_payment_connector::UpdatePaymentConnector,
            update_payment_credential_provider::UpdatePaymentCredentialProvider,
            update_payment_manager::UpdatePaymentManager,
            update_policy::UpdatePolicy,
            update_policy_engine::UpdatePolicyEngine,
            update_registry::UpdateRegistry,
            update_registry_record::UpdateRegistryRecord,
            update_registry_record_status::UpdateRegistryRecordStatus,
            update_workload_identity::UpdateWorkloadIdentity,
        );
        Self {
            gateway_mcp: mcp::GatewayMcpSurface::new(Arc::clone(&engine)),
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            denial: CapacityProviderDenial,
            stated,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way the backend
/// serving that operation states it: the family's value policy, the element constraint sets
/// its own input schema declares, and the value as submitted.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this
/// wrapper does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation. The
/// structurally right place for the collecting half is the frozen crate — handing a
/// handler, or `override_refusal`, the input it already built — which
/// `services/common/src/validation.rs` records as the core seam this re-read exists only
/// until.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// This service's answer to every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk finds, and the wording [`validation`] gives
/// them. `None` when the request breaks none — and for a failure that is NOT a refusal (a
/// body the codec cannot read at all, a member of the wrong type), which the framework's
/// own walk answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_lists`: one read of the request
/// yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input) with each value echo
/// restored to the bytes the caller sent, plus the request's own lists — the value a clause
/// states at a LIST member, which no violation carries. Nothing is re-checked here, and no
/// second copy of that walk lives in this crate: both halves are bounded where those
/// bounds are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    let stated = validation::refusal(
        &violations,
        Some(O::INPUT_SCHEMA),
        validation::echo_of(O::INPUT_SCHEMA),
        &lists,
    )?;
    Some((violations, stated))
}

impl<O> ErasedOperation for StatedPerOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated), req);
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
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
        // This service signs as `bedrock-agentcore` — a signing name it SHARES with the
        // AgentCore DATA plane
        // (`services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore`),
        // whose endpoint is `bedrock-agentcore` while this one's is
        // `bedrock-agentcore-control`. The router resolves a request's routing key from
        // its SigV4 credential scope first, so BOTH services see the key
        // `bedrock-agentcore`: claiming on the signing name alone makes each claim the
        // other's traffic, and the router — which refuses rather than guesses — answers
        // `500 AmbiguousService` for every AgentCore request, a code no AWS model
        // carries.
        //
        // Nor can a path segment separate them: the two models publish the SAME first
        // segment for nine resource families (`/memories/…`, `/runtimes/…`,
        // `/browsers/…`, `/code-interpreters/…`, `/identities/…`,
        // `/capacity-providers/…`, `/harnesses/…`, `/payments/…`,
        // `/browser-profiles/…`), and they collide at deeper indexes too.
        //
        // What separates them is what AWS itself dispatches on: the endpoint host, then
        // the `restJson1` route. So this service claims a request whose routing key names
        // THIS endpoint — its signing name, or the `endpointPrefix` a host-addressed,
        // unsigned request resolves to — and whose method + URI its OWN model binds a
        // route for, by the one rule `service-common` states for every service under a
        // shared signing name. The two models bind no identical method + URI pattern, so
        // exactly one of them claims every AgentCore request.
        if service_common::claims_modeled_route(
            parts,
            &[self.entry.identity().signing_name(), ENDPOINT_PREFIX],
            self.definition(),
        ) {
            return true;
        }
        // And a request addressed to a GATEWAY endpoint host — the host this service
        // PUBLISHES in every gateway's `gatewayUrl`, whose first label is the gateway's
        // own id and which no route key can therefore name. See [`mcp`]: without this the
        // router answers `404 UnknownService` for a URL this service handed out.
        mcp::addresses_gateway_endpoint_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // `ListAgentRuntimeVersionsByCapacityProvider` AUTHORIZES the capacity
        // provider its path names before the request's input is validated: the
        // operation models `AccessDeniedException` (403) and no not-found error, and
        // the recorded service answers 403 `AccessDeniedException` — not 400 — for a
        // `capacityProviderId` that names nothing in the account, including one
        // outside the member's modelled format, which no capacity provider can have.
        //
        // The framework's centralized walk runs the model's constraint validation
        // BEFORE any handler, so a handler cannot take a decision that precedes it;
        // this seam can, which is the only reason the check is reached from here.
        // The rule, and the same refusal for every id that gets past validation,
        // live in the operation's own file. A provider this account holds falls
        // through to the modelled operation below, whose walk validates the request
        // as usual.
        if let Some(id) =
            crate::state::capacity_provider_runtime_versions_id(req.method(), req.uri())
        {
            let unheld = crate::state::service_state(&*self.engine)
                .is_some_and(|state| !state.lock().holds_capacity_provider(&id));
            if unheld {
                return Some(&self.denial as &dyn ErasedOperation);
            }
        }

        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in
        // [`StatedPerOperation`] so a request that breaks a modelled constraint is refused
        // in the words the backend serving THAT operation uses. A key the model does not
        // name resolves to `None` here exactly as it does in the generated table, so the
        // router's routing-error path is unchanged.
        //
        // Ahead of that: a request to a published gateway endpoint host is named as an
        // operation, so the router verifies the credential it carries before anything is
        // served — the router authenticates only what it names an operation for. Nothing
        // in the generated table can name it, because the MCP wire carries no
        // `X-Amz-Target` and this service's model binds no route for its path.
        if mcp::addresses_gateway_endpoint(req) {
            return Some(&self.gateway_mcp);
        }
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. A constraint refusal of a modelled operation is already stated by
        // [`StatedPerOperation`], which runs ahead of this walk; this seam states one that
        // reaches the walk anyway — and it states it through the SAME module, so there is
        // one renderer rather than two that can drift.
        //
        // What it cannot know here is the operation: no input schema, so a collection
        // clause states the constraints that failed rather than the element shape's whole
        // set, and the value policy is the one every family but three shows. Both are the
        // conservative choice for a refusal whose operation is unknown.
        //
        // Authentication is the framework's to word: the request never reached a modelled
        // constraint, so there is no clause of this service's to state.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        validation::refusal(
            violations,
            None,
            validation::Echo::Submitted,
            &HashMap::new(),
        )
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

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a refusal in the framework's wording — and a key spelled differently from
    /// the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_this_services_refusal_wording() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert_eq!(
            operations.len(),
            171,
            "case the model's surface: expected this service's 171 operations, got {}",
            operations.len()
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it \
                 answers a refusal in the framework's wording instead of this service's"
            );
        }
    }
}
