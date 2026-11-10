// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ssm-amazonssm` service: how it claims requests and how it words the
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
//! ## What this service changes about a boundary refusal
//!
//! Two of the five seams are edited here, for one reason: a request this service refuses
//! before any handler runs is worded the way this service words one ([`refusal`], which
//! owns every sentence), and one part of that wording cannot be read off the violations
//! alone.
//!
//! * [`Service::override_refusal`] states the refusal from the violations the framework's
//!   own walk found — the accepted value set of an enumerated type, in the order this
//!   service lists it, with the value half dropped for the types that state none.
//! * [`StatedPerOperation`] states the SAME refusal one step earlier, with the REQUEST in
//!   hand, so the clause stated at a COLLECTION member can quote the collection the caller
//!   submitted. The framework's walk renders a list as its size (`<n> items`), which is
//!   not a value any caller sent; `service_common::validation` re-reads the request and
//!   renders it as the list it was (`[<element>, …]`). Every modelled operation is served
//!   behind it, because a `@length` on a list is a property of an INPUT SHAPE rather than
//!   of one operation.
//!
//! Both paths go through [`refusal::stated_refusal`], so the same request cannot be
//! answered with different words depending on which of them caught it.

mod refusal;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::Violation;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-ssm-amazonssm` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words it.
    /// Keyed by the routing key the protocol resolves — the key the generated table uses —
    /// and built from the SAME shared engine, so a wrapped operation reads and writes the
    /// one store.
    stated: OperationRoutingTable,
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
        // EVERY modelled operation, because a constraint is a property of the INPUT SHAPE
        // and this service's boundary refusal is the same `ValidationException` whichever
        // operation carried the value: an operation whose input declares no constraint
        // simply never has a violation to state, which the wrapper reads off that
        // operation's own input schema. A still-scaffolded operation is wrapped too — the
        // framework refuses a bad request before any handler runs, so its refusals are this
        // service's words even while its success path is a 501. An operation added later is
        // added here as well, or it answers a refusal in the framework's wording rather than
        // this service's; the test at the bottom of this file fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            add_tags_to_resource::AddTagsToResource,
            associate_ops_item_related_item::AssociateOpsItemRelatedItem,
            cancel_command::CancelCommand,
            cancel_maintenance_window_execution::CancelMaintenanceWindowExecution,
            create_activation::CreateActivation,
            create_association::CreateAssociation,
            create_association_batch::CreateAssociationBatch,
            create_cloud_connector::CreateCloudConnector,
            create_document::CreateDocument,
            create_maintenance_window::CreateMaintenanceWindow,
            create_ops_item::CreateOpsItem,
            create_ops_metadata::CreateOpsMetadata,
            create_patch_baseline::CreatePatchBaseline,
            create_resource_data_sync::CreateResourceDataSync,
            delete_activation::DeleteActivation,
            delete_association::DeleteAssociation,
            delete_cloud_connector::DeleteCloudConnector,
            delete_document::DeleteDocument,
            delete_inventory::DeleteInventory,
            delete_maintenance_window::DeleteMaintenanceWindow,
            delete_ops_item::DeleteOpsItem,
            delete_ops_metadata::DeleteOpsMetadata,
            delete_parameter::DeleteParameter,
            delete_parameters::DeleteParameters,
            delete_patch_baseline::DeletePatchBaseline,
            delete_resource_data_sync::DeleteResourceDataSync,
            delete_resource_policy::DeleteResourcePolicy,
            deregister_managed_instance::DeregisterManagedInstance,
            deregister_patch_baseline_for_patch_group::DeregisterPatchBaselineForPatchGroup,
            deregister_target_from_maintenance_window::DeregisterTargetFromMaintenanceWindow,
            deregister_task_from_maintenance_window::DeregisterTaskFromMaintenanceWindow,
            describe_activations::DescribeActivations,
            describe_association::DescribeAssociation,
            describe_association_execution_targets::DescribeAssociationExecutionTargets,
            describe_association_executions::DescribeAssociationExecutions,
            describe_automation_executions::DescribeAutomationExecutions,
            describe_automation_step_executions::DescribeAutomationStepExecutions,
            describe_available_patches::DescribeAvailablePatches,
            describe_document::DescribeDocument,
            describe_document_permission::DescribeDocumentPermission,
            describe_effective_instance_associations::DescribeEffectiveInstanceAssociations,
            describe_effective_patches_for_patch_baseline::DescribeEffectivePatchesForPatchBaseline,
            describe_instance_associations_status::DescribeInstanceAssociationsStatus,
            describe_instance_information::DescribeInstanceInformation,
            describe_instance_patch_states::DescribeInstancePatchStates,
            describe_instance_patch_states_for_patch_group::DescribeInstancePatchStatesForPatchGroup,
            describe_instance_patches::DescribeInstancePatches,
            describe_instance_properties::DescribeInstanceProperties,
            describe_inventory_deletions::DescribeInventoryDeletions,
            describe_maintenance_window_execution_task_invocations::DescribeMaintenanceWindowExecutionTaskInvocations,
            describe_maintenance_window_execution_tasks::DescribeMaintenanceWindowExecutionTasks,
            describe_maintenance_window_executions::DescribeMaintenanceWindowExecutions,
            describe_maintenance_window_schedule::DescribeMaintenanceWindowSchedule,
            describe_maintenance_window_targets::DescribeMaintenanceWindowTargets,
            describe_maintenance_window_tasks::DescribeMaintenanceWindowTasks,
            describe_maintenance_windows::DescribeMaintenanceWindows,
            describe_maintenance_windows_for_target::DescribeMaintenanceWindowsForTarget,
            describe_ops_items::DescribeOpsItems,
            describe_parameters::DescribeParameters,
            describe_patch_baselines::DescribePatchBaselines,
            describe_patch_group_state::DescribePatchGroupState,
            describe_patch_groups::DescribePatchGroups,
            describe_patch_properties::DescribePatchProperties,
            describe_sessions::DescribeSessions,
            disassociate_ops_item_related_item::DisassociateOpsItemRelatedItem,
            get_access_token::GetAccessToken,
            get_automation_execution::GetAutomationExecution,
            get_calendar_state::GetCalendarState,
            get_cloud_connector::GetCloudConnector,
            get_command_invocation::GetCommandInvocation,
            get_connection_status::GetConnectionStatus,
            get_default_patch_baseline::GetDefaultPatchBaseline,
            get_deployable_patch_snapshot_for_instance::GetDeployablePatchSnapshotForInstance,
            get_document::GetDocument,
            get_execution_preview::GetExecutionPreview,
            get_inventory::GetInventory,
            get_inventory_schema::GetInventorySchema,
            get_maintenance_window::GetMaintenanceWindow,
            get_maintenance_window_execution::GetMaintenanceWindowExecution,
            get_maintenance_window_execution_task::GetMaintenanceWindowExecutionTask,
            get_maintenance_window_execution_task_invocation::GetMaintenanceWindowExecutionTaskInvocation,
            get_maintenance_window_task::GetMaintenanceWindowTask,
            get_ops_item::GetOpsItem,
            get_ops_metadata::GetOpsMetadata,
            get_ops_summary::GetOpsSummary,
            get_parameter::GetParameter,
            get_parameter_history::GetParameterHistory,
            get_parameters::GetParameters,
            get_parameters_by_path::GetParametersByPath,
            get_patch_baseline::GetPatchBaseline,
            get_patch_baseline_for_patch_group::GetPatchBaselineForPatchGroup,
            get_resource_policies::GetResourcePolicies,
            get_service_setting::GetServiceSetting,
            label_parameter_version::LabelParameterVersion,
            list_association_versions::ListAssociationVersions,
            list_associations::ListAssociations,
            list_cloud_connectors::ListCloudConnectors,
            list_command_invocations::ListCommandInvocations,
            list_commands::ListCommands,
            list_compliance_items::ListComplianceItems,
            list_compliance_summaries::ListComplianceSummaries,
            list_document_metadata_history::ListDocumentMetadataHistory,
            list_document_versions::ListDocumentVersions,
            list_documents::ListDocuments,
            list_inventory_entries::ListInventoryEntries,
            list_nodes::ListNodes,
            list_nodes_summary::ListNodesSummary,
            list_ops_item_events::ListOpsItemEvents,
            list_ops_item_related_items::ListOpsItemRelatedItems,
            list_ops_metadata::ListOpsMetadata,
            list_resource_compliance_summaries::ListResourceComplianceSummaries,
            list_resource_data_sync::ListResourceDataSync,
            list_tags_for_resource::ListTagsForResource,
            modify_document_permission::ModifyDocumentPermission,
            put_compliance_items::PutComplianceItems,
            put_inventory::PutInventory,
            put_parameter::PutParameter,
            put_resource_policy::PutResourcePolicy,
            register_default_patch_baseline::RegisterDefaultPatchBaseline,
            register_patch_baseline_for_patch_group::RegisterPatchBaselineForPatchGroup,
            register_target_with_maintenance_window::RegisterTargetWithMaintenanceWindow,
            register_task_with_maintenance_window::RegisterTaskWithMaintenanceWindow,
            remove_tags_from_resource::RemoveTagsFromResource,
            reset_service_setting::ResetServiceSetting,
            resume_session::ResumeSession,
            send_automation_signal::SendAutomationSignal,
            send_command::SendCommand,
            start_access_request::StartAccessRequest,
            start_associations_once::StartAssociationsOnce,
            start_automation_execution::StartAutomationExecution,
            start_change_request_execution::StartChangeRequestExecution,
            start_execution_preview::StartExecutionPreview,
            start_session::StartSession,
            stop_automation_execution::StopAutomationExecution,
            terminate_session::TerminateSession,
            unlabel_parameter_version::UnlabelParameterVersion,
            update_association::UpdateAssociation,
            update_association_status::UpdateAssociationStatus,
            update_cloud_connector::UpdateCloudConnector,
            update_document::UpdateDocument,
            update_document_default_version::UpdateDocumentDefaultVersion,
            update_document_metadata::UpdateDocumentMetadata,
            update_maintenance_window::UpdateMaintenanceWindow,
            update_maintenance_window_target::UpdateMaintenanceWindowTarget,
            update_maintenance_window_task::UpdateMaintenanceWindowTask,
            update_managed_instance_role::UpdateManagedInstanceRole,
            update_ops_item::UpdateOpsItem,
            update_ops_metadata::UpdateOpsMetadata,
            update_patch_baseline::UpdatePatchBaseline,
            update_resource_data_sync::UpdateResourceDataSync,
            update_service_setting::UpdateServiceSetting,
            validate_cloud_connector::ValidateCloudConnector,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated,
        }
    }
}

/// One modelled operation, served so that a request refused before its handler runs is
/// stated the way this service states it: a collection as the collection it sent, and an
/// enumerated type's value set as this service lists it.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this can neither refuse a request the framework's
/// walk would accept nor accept one it would refuse. The cost is that a request it does not
/// refuse is read twice, once here and once in the operation's own sequence, because the
/// framework hands no already-built input to an operation; the structurally right place for
/// the collecting half is the frozen crate, which `services/common/src/validation.rs`
/// records as the core seam this re-read exists only until (its header names the filed gap:
/// `issues/things-that-do-not-work/20261022T133000Z-gap-smithy-schema-server-constraint-violation-stores-only-the-elided-rejected-value.md`).
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// How this service states every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk found, and the refusal [`refusal`] words them as.
/// `None` when the request breaks none — and for a failure that is NOT a refusal (a body the
/// codec cannot read at all, a member of the wrong type), which the framework's own walk
/// answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_echoes`: one read of the request
/// yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input), each paired with how a
/// clause can state the value it is about — which for a COLLECTION member is the collection
/// itself, the one thing a violation does not carry. Nothing is re-checked here and no
/// second copy of that walk lives in this crate: both halves are bounded where those bounds
/// are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let refused =
        ::service_common::validation::refusing_violations_with_echoes::<O>(protocol, req);
    let violations: Vec<Violation> = refused
        .iter()
        .map(|(violation, _)| violation.clone())
        .collect();
    let stated = refusal::validation_refusal_quoting_collections(O::NAME, refused)?;
    Some((violations, stated))
}

/// Record a refusal answered above as the operation row the framework sequence would have
/// recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and the error shape it names), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already sanitized request metadata. Everything is gated on a client collecting, exactly
/// as the framework gates it.
fn record_refusal(
    ctx: &OperationContext<'_>,
    operation: &'static str,
    error_code: &str,
    response: &RtResponse,
    latency: std::time::Duration,
) {
    if !::emulator_telemetry::recording_active() {
        return;
    }
    let handle = ::emulator_telemetry::OperationHandle::folded(
        ctx.identity().signing_name_static(),
        operation,
    );
    ::emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(::emulator_telemetry::OperationFailure::classify(
            response.status().as_u16(),
            error_code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers)
            .into_metadata(),
    );
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
            let started = std::time::Instant::now();
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                let code = stated.code.clone();
                // The SAME renderer the centralized walk would have reached — this
                // service's own pre-dispatch envelope and the status the protocol gives a
                // refusal — with this service's code and message in place of the
                // framework's.
                let response = svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated), req);
                record_refusal(ctx, O::NAME, &code, &response, started.elapsed());
                return response;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in `StatedPerOperation` so
        // a refused request is worded the way this service words it. A key the model does
        // not name resolves to `None` here exactly as it does in the generated table, so the
        // router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service reword it. The
        // code is the one the framework's own default picks for a broken constraint
        // (`ValidationException`, 400) and travels with the message, so a client branches on
        // the same code either way.
        //
        // A refusal of a request that named a modelled operation is answered by
        // `StatedPerOperation` before this walk is reached, with the REQUEST in hand. What
        // still arrives here is a refusal with no modelled operation to read the request
        // against; it is worded by the same module, so the two paths cannot state one
        // request differently — this one just has only the violations to state from, so a
        // clause stated at a collection member quotes the framework's own size summary.
        refusal::stated_refusal(refusal)
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(&self, error: &ProtocolError) -> Option<RtResponse> {
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
        "/smithy-model/com-amazonaws-ssm-amazonssm.json"
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
        assert!(
            operations.len() > 100,
            "case the model's surface: expected this service's whole operation list, got \
             {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it answers \
                 a refusal in the framework's wording instead of this service's"
            );
        }
    }
}
