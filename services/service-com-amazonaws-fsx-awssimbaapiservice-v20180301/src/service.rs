// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-fsx-awssimbaapiservice-v20180301` service: how it claims requests and how it words the
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
//! # How this service refuses a request it never ran
//!
//! Amazon FSx answers a request whose input violates a modelled constraint with
//! `BadRequest` — one of the two errors the model binds to EVERY operation
//! (`smithy_query(query="digest")` reports `common_errors: ["BadRequest",
//! "InternalServerError"]`) — at HTTP 400, with the standard constraint-violation
//! envelope in the `Message` member that shape declares (a value no modelled type can be
//! read from is refused one layer earlier and states that layer's generic message). The
//! FSx model declares no `ValidationException` at all, so the framework's cross-service
//! pre-dispatch envelope names an error no FSx client can match and puts its prose in a
//! lowercase `message` key no FSx error shape uses.
//!
//! Every operation is therefore served through [`InModelledEnvelope`], which states that
//! refusal itself — from the framework's own violations, plus the `@pattern`s the
//! framework's regex engine cannot compile and so silently passes — before the handler
//! runs and therefore before the request's resources are interpreted. `validation` owns
//! the wording and the pattern check, and `override_refusal` words any refusal the
//! framework still renders the same way so the two paths cannot disagree.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The constraint-refusal layer: how this service words a request it never ran, and the
/// modelled `@pattern`s it has to check itself. Internal to this crate — what a client
/// sees of it is the `BadRequest` on the wire, which the crate's tests assert through the
/// router.
mod validation;

/// The `com-amazonaws-fsx-awssimbaapiservice-v20180301` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before its handler runs is refused in
    /// this service's own `BadRequest`. Keyed by the routing key the protocol resolves —
    /// the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
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
        // EVERY operation of the model, because `BadRequest` is a service-COMMON error:
        // the constraint refusal each one owes is the same refusal, so none of them may
        // answer it in a different envelope. The list is hand-maintained (see the
        // `TO CONSOLIDATE` note on `InModelledEnvelope`): an operation added to the model
        // and not added here would fall back to the framework's envelope, so the
        // `no_operation_answers_a_foreign_validation_exception` test in
        // `tests/all/constraint_refusals.rs` names every operation the model declares and
        // is what catches the omission.
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            associate_file_system_aliases::AssociateFileSystemAliases,
            cancel_data_repository_task::CancelDataRepositoryTask,
            copy_backup::CopyBackup,
            copy_snapshot_and_update_volume::CopySnapshotAndUpdateVolume,
            create_and_attach_s3_access_point::CreateAndAttachS3AccessPoint,
            create_backup::CreateBackup,
            create_data_repository_association::CreateDataRepositoryAssociation,
            create_data_repository_task::CreateDataRepositoryTask,
            create_file_cache::CreateFileCache,
            create_file_system::CreateFileSystem,
            create_file_system_from_backup::CreateFileSystemFromBackup,
            create_snapshot::CreateSnapshot,
            create_storage_virtual_machine::CreateStorageVirtualMachine,
            create_volume::CreateVolume,
            create_volume_from_backup::CreateVolumeFromBackup,
            delete_backup::DeleteBackup,
            delete_data_repository_association::DeleteDataRepositoryAssociation,
            delete_file_cache::DeleteFileCache,
            delete_file_system::DeleteFileSystem,
            delete_snapshot::DeleteSnapshot,
            delete_storage_virtual_machine::DeleteStorageVirtualMachine,
            delete_volume::DeleteVolume,
            describe_backups::DescribeBackups,
            describe_data_repository_associations::DescribeDataRepositoryAssociations,
            describe_data_repository_tasks::DescribeDataRepositoryTasks,
            describe_file_caches::DescribeFileCaches,
            describe_file_system_aliases::DescribeFileSystemAliases,
            describe_file_systems::DescribeFileSystems,
            describe_s3_access_point_attachments::DescribeS3AccessPointAttachments,
            describe_shared_vpc_configuration::DescribeSharedVpcConfiguration,
            describe_snapshots::DescribeSnapshots,
            describe_storage_virtual_machines::DescribeStorageVirtualMachines,
            describe_volumes::DescribeVolumes,
            detach_and_delete_s3_access_point::DetachAndDeleteS3AccessPoint,
            disassociate_file_system_aliases::DisassociateFileSystemAliases,
            list_tags_for_resource::ListTagsForResource,
            release_file_system_nfs_v3_locks::ReleaseFileSystemNfsV3Locks,
            restore_volume_from_snapshot::RestoreVolumeFromSnapshot,
            start_misconfigured_state_recovery::StartMisconfiguredStateRecovery,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_data_repository_association::UpdateDataRepositoryAssociation,
            update_file_cache::UpdateFileCache,
            update_file_system::UpdateFileSystem,
            update_shared_vpc_configuration::UpdateSharedVpcConfiguration,
            update_snapshot::UpdateSnapshot,
            update_storage_virtual_machine::UpdateStorageVirtualMachine,
            update_volume::UpdateVolume,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the constraint refusal it owes stated in this
/// service's own `BadRequest`.
///
/// The framework's pre-dispatch renderer gets the status (400) right and nothing else
/// for FSx: it names `ValidationException`, a shape this model does not declare, and
/// writes the prose into a framework-chosen lowercase `message` key, while every FSx
/// error shape declares its prose member as `Message`. An SDK therefore degrades the
/// refusal to an unmodelled error and surfaces a null reason.
///
/// So this wrapper collects the refusal itself — the very classes the framework's own walk
/// collects, from the framework's own deserializer and constraint walk, plus the
/// `@pattern`s that walk cannot compile — and renders it through the model's `BadRequest`
/// schema. Nothing else is re-implemented: a request it does not refuse is handed to the
/// generated `ErasedOperation::run`, which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. It is also the one place every
/// operation's input is seen before its handler, which is why the pattern check lives
/// here rather than in the handlers that read those members.
///
/// TO CONSOLIDATE, in the framework rather than here: four other services carry this
/// same wrapper (`costoptimizationhub`, `directoryservicedata`, `managedblockchain`,
/// `partnercentralaccount`, and `uxc` / `networkmonitor` carry the `wrap_operations!`
/// half), because the only seam a service has — `Service::override_refusal` — can choose the
/// error CODE and the prose but not the member key the pre-dispatch renderer writes them
/// into. Letting that renderer serialize through a service's own modelled error shape
/// (or widening the seam to carry the shape and its prose member) would delete every one
/// of those copies, and with them the second deserialize each one pays per request: this
/// wrapper reads the input to find the refusal and the generated `run` reads it again to
/// handle the request. It would also make the wrapping exhaustive BY CONSTRUCTION, which the
/// hand-maintained list in `wrap_operations!` below is not: `OperationRoutingTable`
/// exposes only `add_operation` / `get`, so this crate cannot walk the generated table to
/// wrap whatever it holds. Both live under `vendor/`, which a service crate may not edit.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
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
            // Scoped so the borrowed `&dyn ErasedServiceDefinition` — which is not
            // `Sync`, while this future is — is dropped before the await below. The
            // protocol itself is.
            let refusal = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                validation::refusal_for::<O>(protocol, req)
                    .and_then(|message| validation::bad_request(protocol, message))
            };
            if let Some(refusal) = refusal {
                return refusal;
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused before the handler runs is refused
        // in this service's `BadRequest`, which names a shape an FSx client matches and
        // carries the `Message` member the model declares, rather than in the
        // framework's pre-dispatch envelope, which names neither. A key the model does
        // not name resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam words any refusal the FRAMEWORK still renders — a
        // request whose key reached no wrapped operation, and the fallback in
        // `validation::bad_request`. Wording it from the same violations keeps the two
        // paths from disagreeing: a value outside a declared constraint is this service's
        // `BadRequest` either way. An authentication refusal is not this service's to
        // reword, so it keeps the framework's 403.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        Some(Refused {
            code: validation::BAD_REQUEST.to_string(),
            message: validation::refusal_message(violations)?,
        })
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
