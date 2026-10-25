// SPDX-License-Identifier: CC0-1.0

//! @op CreatePersistentAppUI
//!
//! Mints a persistent application user interface for the resource `TargetResourceArn`
//! names and persists it, so `DescribePersistentAppUI` and
//! `GetPersistentAppUIPresignedURL` report what this call stored.
//!
//! A `TargetResourceArn` that is not an ARN, or is an ARN of a service Amazon EMR
//! publishes no application user interface for, is refused with HTTP 400
//! `ValidationException` — the error code the service answers a rejected input on this
//! operation with — and so is an EMR on EC2 cluster ARN this account holds no cluster
//! for: there is then no resource to attach a UI to. Nothing is stored on any of those
//! paths.
//!
//! `RuntimeRoleEnabledCluster` is COMPUTED from the target cluster's own security
//! configuration (see
//! [`crate::state::ComAmazonawsEmrElasticmapreduceState::is_runtime_role_enabled_cluster`]),
//! never taken from the request.
//!
//! `EMRContainersConfig` and `XReferer` are accepted and not stored: no member of
//! `PersistentAppUIOutput`, of the `PersistentAppUI` shape `DescribePersistentAppUI`
//! answers with, or of `GetPersistentAppUIPresignedURLOutput` reports either of them, so
//! nothing this service can be asked afterwards is computed from them. A test must not
//! expect them to be echoed back.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreatePersistentAppUI::new(engine)` and drives it through the centralized request walk:
//! deserialize -> validate -> `handle` -> serialize.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The
//! store is reached through [`crate::state::service_state`].
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePersistentAppUI`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` ...`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json")
//! ```
//!
//! Creates a persistent application user interface.
//!
//! ## Input
//!
//! * `EMRContainersConfig` (EMRContainersConfig) - The EMR containers configuration.
//! * `ProfilerType` (ProfilerType, one of: SHS | TEZUI | YTS) - The profiler type for the
//!   persistent application user interface.
//! * `Tags` (TagList) - Tags for the persistent application user interface.
//! * `TargetResourceArn` (ArnType, len 20..2048) **required** - The unique Amazon Resource
//!   Name (ARN) of the target resource.
//! * `XReferer` (String) - The cross reference for the persistent application user interface.
//!
//! ## Output
//!
//! * `PersistentAppUIId` (XmlStringMaxLen256, len 0..256) - The persistent application user
//!   interface identifier.
//! * `RuntimeRoleEnabledCluster` (Boolean) - Represents if the EMR on EC2 cluster that the
//!   persisent application user interface is created for is a runtime role enabled cluster or
//!   not.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) - This exception occurs when there is an internal
//!   failure in the Amazon EMR service.
//! * `InvalidRequestException` (HTTP 400) - This exception occurs when there is something
//!   wrong with user input.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePersistentAppUIError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-emr-elasticmapreduce.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePersistentAppUIError` alias below.
#[smithy_macros::operation_error("com.amazonaws.emr#CreatePersistentAppUI")]
#[derive(Debug)]
pub enum CreatePersistentAppUIErrorModeled {
    InternalServerException(InternalServerException),
    InvalidRequestException(InvalidRequestException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_persistent_app_ui::CreatePersistentAppUIError`).
pub type CreatePersistentAppUIError = crate::scaffolded::Scaffolded<CreatePersistentAppUIErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<CreatePersistentAppUIErrorModeled> for CreatePersistentAppUIError {
    fn from(modeled: CreatePersistentAppUIErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePersistentAppUI` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct CreatePersistentAppUI<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePersistentAppUI<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePersistentAppUI<Eng>
{
    type Input = CreatePersistentAppUIInput;
    type Output = CreatePersistentAppUIOutput;
    type Error = CreatePersistentAppUIError;

    const NAME: &'static str = "CreatePersistentAppUI";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PERSISTENT_APP_UI_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PERSISTENT_APP_UI_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::{
            persistent_app_ui_status, AuditOutcome, PersistentAppUIRecord, MAX_PERSISTENT_APP_UIS,
            TRANSITION_SPAN,
        };

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());

        // Minting a user interface CHANGES this service's state, so every branch that
        // returns from this call leaves one structured access record naming the
        // operation, the resource, the presented access key id, the request target and
        // the outcome (`crate::state::audit`). A refusal has no minted id to name yet,
        // so it records the target the request asked for.
        let requested_target = input.target_resource_arn.clone();

        // Every refusal below is taken BEFORE anything is stored, so a refused create
        // leaves no UI behind. The refusal is a 400 `ValidationException`: that is the
        // error code Amazon EMR answers a rejected input on this operation with, which
        // the operation's own model does not declare (it carries only
        // `InvalidRequestException`), so it goes through the channel that can render it.
        let refuse = |message: String| -> Self::Error {
            crate::state::audit(
                Self::NAME,
                &requested_target,
                ctx,
                AuditOutcome::ValidationRefused,
            );
            crate::scaffolded::Scaffolded::validation(message)
        };

        // A target resource is named by its ARN, read through the tree's one
        // caller-supplied-ARN parse: a value that does not split into ARN fields, one
        // whose vendor publishes no application user interface, and one of this
        // service's own vendor naming anything but a cluster all name no resource this
        // operation can attach a UI to. They are the same rejection of the same member,
        // so they answer the same refusal, naming the value sent.
        if !crate::state::is_persistent_app_ui_target_arn(&input.target_resource_arn) {
            return Err(refuse(crate::state::not_valid_message(
                &input.target_resource_arn,
            )));
        }
        let cluster_id = crate::state::arn_cluster_id(&input.target_resource_arn).map(str::to_string);
        if crate::state::is_own_vendor_arn(&input.target_resource_arn) && cluster_id.is_none() {
            // EMR publishes an application user interface for a CLUSTER, so one of its
            // own ARNs naming a studio, a notebook execution or no resource id at all
            // names nothing to attach a UI to.
            return Err(refuse(crate::state::not_valid_message(
                &input.target_resource_arn,
            )));
        }

        // The UI publishes the profiler the request asked for, in the vocabulary the UI
        // is addressed by later (`PersistentAppUIType`, which spells the Tez UI `TEZ`).
        // The modelled enum constraint already refuses any other value at the boundary;
        // the refusal below is what keeps this mapping total rather than guessing a UI
        // type for a profiler the model does not declare.
        let ui_type = match input.profiler_type.as_deref() {
            None => crate::state::DEFAULT_PERSISTENT_APP_UI_TYPE,
            Some(profiler) => match crate::state::persistent_app_ui_type_of_profiler(profiler) {
                Some(ui_type) => ui_type,
                None => {
                    return Err(refuse(format!(
                        "ProfilerType '{profiler}' is not valid; it must be one of {}.",
                        PROFILER_TYPE_VALUES.join(", ")
                    )))
                }
            },
        };

        let Some(state) = crate::state::service_state(&*self.engine) else {
            crate::state::audit(
                Self::NAME,
                &requested_target,
                ctx,
                AuditOutcome::StoreUnavailable,
            );
            return Err(CreatePersistentAppUIErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "the EMR persistent application user interface store is unavailable"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let mut store = state.lock();

        // An EMR on EC2 target is a cluster this account holds: a UI cannot be attached
        // to a cluster that does not exist, so the id is resolved rather than taken on
        // trust.
        if let Some(cluster_id) = cluster_id.as_deref() {
            if !store.clusters.contains_key(cluster_id) {
                return Err(refuse(crate::state::unknown_cluster_message(cluster_id)));
            }
        }

        // "Represents if the EMR on EC2 cluster that the persisent application user
        // interface is created for is a runtime role enabled cluster or not."
        // (`CreatePersistentAppUIOutput$RuntimeRoleEnabledCluster` doc.) Computed from
        // the target cluster's own security configuration; a target that is not an EMR
        // on EC2 cluster has no such cluster, so it is not one.
        let runtime_role_enabled_cluster = cluster_id
            .as_deref()
            .is_some_and(|cluster_id| store.is_runtime_role_enabled_cluster(cluster_id));

        // Past the account's ceiling the create is refused and nothing is stored, the
        // way a quota is.
        if store.persistent_app_uis.len() >= MAX_PERSISTENT_APP_UIS {
            crate::state::audit(
                Self::NAME,
                &requested_target,
                ctx,
                AuditOutcome::QuotaExceeded,
            );
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "This account already holds {MAX_PERSISTENT_APP_UIS} persistent application \
                 user interfaces, which is the maximum."
            )));
        }

        let id = store.mint_persistent_app_ui_id();
        let record = PersistentAppUIRecord {
            id: id.clone(),
            target_resource_arn: input.target_resource_arn,
            cluster_id,
            runtime_role_enabled_cluster,
            ui_types: vec![ui_type.to_string()],
            region,
            // The service has to attach the UI to its target before it serves, so it is
            // not attached yet and the clock owes it that move.
            status: persistent_app_ui_status::STARTING.to_string(),
            // "The author ID for the persistent application user interface object."
            // (`PersistentAppUI$AuthorId` doc): the principal this request was signed
            // by, which is the only identity this emulator knows.
            author_id: crate::state::caller_arn(ctx.parts()),
            creation_time: now,
            last_modified_time: now,
            last_state_change_reason: None,
            tags: input.tags.unwrap_or_default(),
            attached_due_at: Some(now + TRANSITION_SPAN),
        };

        // What is answered is what was stored, so a caller sees exactly what a later
        // `DescribePersistentAppUI` reports.
        store.persistent_app_uis.insert(id.clone(), record);
        crate::state::audit(Self::NAME, &id, ctx, AuditOutcome::PersistentAppUICreated);

        Ok(CreatePersistentAppUIOutput {
            persistent_app_ui_id: Some(id),
            runtime_role_enabled_cluster: Some(runtime_role_enabled_cluster),
        })
    }
}
