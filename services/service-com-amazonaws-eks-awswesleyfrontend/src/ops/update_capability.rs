// SPDX-License-Identifier: CC0-1.0

//! @op UpdateCapability
//! IMPLEMENTED. Starts the asynchronous update of one managed capability of one
//! cluster and answers with the `Update` that tracks it: "When you update a
//! capability, Amazon EKS applies the changes and may restart capability components
//! as needed." The requested values are NOT on the capability the instant the call
//! returns — they wait on the record as a
//! [`crate::state::PendingCapabilityUpdate`] and are written onto it when the clock
//! completes the update, which is also when the capability leaves `UPDATING` for
//! `ACTIVE`. `DescribeCapability` and `ListCapabilities` read that one record, and
//! `DescribeUpdate` and `ListUpdates` read the update this mints.
//!
//! The rules it enforces:
//!
//! * both `clusterName` and `capabilityName` are `@httpLabel`s addressing ONE
//!   resource, so a miss of either half is the `ResourceNotFoundException`
//!   `DescribeCapability` and `DeleteCapability` give for the same pair;
//! * `configuration`'s "structure ... varies depending on the capability type"
//!   (`CapabilityConfigurationRequest`), so a configuration member belonging to
//!   another `CapabilityType` than the stored one is `InvalidParameterException`;
//! * a capability can only take a change once it has SETTLED, and only on a running
//!   control plane, so a capability or cluster outside those statuses is refused;
//! * one capability takes one update at a time, so a second update of it while the
//!   first is in flight is `ResourceInUseException`;
//! * `clientRequestToken` is `@idempotencyToken`, so a retry carrying the token of
//!   the call that started the update is answered with that same update.
//!
//! ORDER: the pass-role authorization of `roleArn` (403), which is decided from the
//! value alone and names no resource of this service, comes first — as it does in
//! `CreateCapability`. The two resources the request addresses are resolved next,
//! before any member of the body is judged and before `roleArn` is resolved against
//! IAM, so an update against a capability that does not exist reports the missing
//! resource rather than a complaint about a sibling member.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//!
//! ---
//!
//! # Modeled contract for `UpdateCapability`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates the configuration of a managed capability in your Amazon EKS cluster. You can
//! update the IAM role, configuration settings, and delete propagation policy for a
//! capability. When you update a capability, Amazon EKS applies the changes and may restart
//! capability components as needed. The capability remains available during the update
//! process, but some operations may be temporarily unavailable.
//!
//! ## Input
//!
//! * `capabilityName` (String) **required** — The name of the capability to update
//!   configuration for.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request. This token is valid for 24
//!   hours after creation.
//! * `clusterName` (String) **required** — The name of the Amazon EKS cluster that contains
//!   the capability you want to update configuration for.
//! * `configuration` (UpdateCapabilityConfiguration) — The updated configuration settings for
//!   the capability. You only need to specify the configuration parameters you want to
//!   change. For Argo CD capabilities, you can update RBAC role mappings and network access
//!   settings.
//! * `deletePropagationPolicy` (CapabilityDeletePropagationPolicy, one of: RETAIN) — The
//!   updated delete propagation policy for the capability. Currently, the only supported
//!   value is RETAIN.
//! * `roleArn` (String) — The Amazon Resource Name (ARN) of the IAM role that the capability
//!   uses to interact with Amazon Web Services services. If you specify a new role ARN, the
//!   capability will start using the new role for all subsequent operations.
//!
//! ## Output
//!
//! * `update` (Update) — An object representing an asynchronous update.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, PendingCapabilityUpdate, UpdateRecord, UpdateScope, CAPABILITY_ACTIVATION_DELAY,
    CAPABILITY_CONFIGURATION_MISMATCH_MESSAGE, CAPABILITY_ROLE_PRINCIPALS,
    CAPABILITY_STATUSES_AN_UPDATE_STARTS_FROM, CAPABILITY_STATUS_UPDATING, CAPABILITY_TYPE_ACK,
    CAPABILITY_TYPE_ARGOCD, STATUS_ACTIVE, UPDATE_IN_PROGRESS,
    UPDATE_PARAM_DISABLED_SERVICES, UPDATE_PARAM_ENABLE_CROSS_NAMESPACE,
    UPDATE_PARAM_NETWORK_ACCESS, UPDATE_PARAM_ROLE_ARN,
    UPDATE_PARAM_ROLE_MAPPINGS_TO_ADD_OR_UPDATE, UPDATE_PARAM_ROLE_MAPPINGS_TO_REMOVE,
    UPDATE_TYPE_CAPABILITY,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateCapabilityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateCapabilityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateCapability")]
#[derive(Debug)]
pub enum UpdateCapabilityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidParameterException(InvalidParameterException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_capability::UpdateCapabilityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateCapabilityError = crate::scaffolded::Scaffolded<UpdateCapabilityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateCapabilityErrorModeled> for UpdateCapabilityError {
    fn from(modeled: UpdateCapabilityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateCapability` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateCapability<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateCapability<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateCapability<Eng>
{
    type Input = UpdateCapabilityInput;
    type Output = UpdateCapabilityOutput;
    type Error = UpdateCapabilityError;

    const NAME: &'static str = "UpdateCapability";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CAPABILITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CAPABILITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateCapabilityError::from(UpdateCapabilityErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This call addresses ONE resource — the `(cluster, capability)` pair — so a
        // miss of either half is the service's generic, memberless not-found payload,
        // exactly as `DescribeCapability` and `DeleteCapability` answer it
        // ([`crate::state::resource_not_found`]).
        let not_found = || {
            UpdateCapabilityError::from(UpdateCapabilityErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            ))
        };
        let invalid_parameter = |message: String| {
            UpdateCapabilityError::from(UpdateCapabilityErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        let now = SystemTime::now();

        // AUTHORIZATION FIRST, as `CreateCapability` authorizes the same member: a
        // `roleArn` is a role the capability has Amazon EKS assume, so whether THIS
        // caller may hand it over at all is decided from the value alone
        // ([`state::pass_role_refusal`]) — a permission question that needs no state
        // and precedes every report about the resources the request names. A request
        // that names no new role asks for no pass-role at all.
        if let Some(why) = input
            .role_arn
            .as_deref()
            .and_then(state::pass_role_refusal)
        {
            return Err(UpdateCapabilityErrorModeled::AccessDeniedException(
                AccessDeniedException { message: Some(why) },
            )
            .into());
        }

        // THE RESOURCES THE REQUEST ADDRESSES NEXT: both `clusterName` and
        // `capabilityName` are `@httpLabel`s, a capability hangs off ONE cluster, and a
        // cluster name identifies a cluster only within one region, so both are
        // resolved in the region the request is signed for before any member of the
        // body is judged against them.
        //
        // The lock is taken for the resolution alone and released again, because the
        // IAM call below is an `await` and this store's guard is not held across one.
        let resolved = {
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);
            // A cluster `RegisterCluster` connected has no Amazon EKS-managed control
            // plane and so can hold no capability at all: the lookup below is what
            // reports that, with the same 404 any other missing capability earns,
            // rather than a gate of its own.
            if !guard
                .clusters
                .contains_key(&(region.clone(), input.cluster_name.clone()))
            {
                return Err(not_found());
            }
            guard
                .capabilities
                .get(&(
                    region.clone(),
                    input.cluster_name.clone(),
                    input.capability_name.clone(),
                ))
                .map(|capability| (capability.r#type.clone(), capability.status.clone()))
        };
        let Some((capability_type, capability_status)) = resolved else {
            return Err(not_found());
        };

        // The resources resolved, so a role the body names can be resolved too: IAM
        // holds the role and its trust policy must let the Amazon EKS service principal
        // assume it, the same rule `CreateCapability` holds the role it is created with
        // to ([`state::service_principal_trust_violation`]). It runs with no lock held:
        // it is a call out to another service.
        if let Some(role_arn) = input.role_arn.as_deref() {
            if let Some(why) = state::service_principal_trust_violation(
                ctx,
                &region,
                role_arn,
                CAPABILITY_ROLE_PRINCIPALS,
            )
            .await
            {
                return Err(invalid_parameter(format!("roleArn {why}")));
            }
        }

        // "The structure of this object varies depending on the capability type"
        // (`CapabilityConfigurationRequest`), and an update carries the same
        // per-type structure: each member of `configuration` belongs to exactly one
        // `CapabilityType`, so a configuration the capability's type has no use for is
        // a bad parameter rather than something to store and report back unused. The
        // type is the STORED one — a capability's type is fixed when it is created and
        // this operation does not change it.
        if let Some(configuration) = input.configuration.as_ref() {
            for (owning_type, supplied) in [
                (CAPABILITY_TYPE_ARGOCD, configuration.argo_cd.is_some()),
                (CAPABILITY_TYPE_ACK, configuration.ack.is_some()),
            ] {
                if supplied && capability_type != owning_type {
                    return Err(invalid_parameter(
                        CAPABILITY_CONFIGURATION_MISMATCH_MESSAGE.to_string(),
                    ));
                }
            }
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The lock was released for the IAM call, so both resources are resolved again
        // — and a delete that landed in between is reported the same way as before.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(not_found());
        };
        let cluster_status = cluster.status.clone();
        let capability_key = (
            region.clone(),
            input.cluster_name.clone(),
            input.capability_name.clone(),
        );
        if !guard.capabilities.contains_key(&capability_key) {
            return Err(not_found());
        }

        // An update of a capability is an update OF THAT CAPABILITY: it is what selects
        // the update in a listing (`ListUpdates$capabilityName`) and what makes a
        // second update of the same capability a conflict while the first is running.
        let scope = UpdateScope::Capability(input.capability_name.clone());

        // A retry. `clientRequestToken` is `@idempotencyToken`: an SDK generates it
        // once per call and RESENDS the same value when it retries, so a retry of the
        // call that started an update of this capability is answered with THAT update
        // rather than starting a second one. A blank token identifies nothing, so it
        // never matches, and only an update of this capability can be a retry of this
        // call.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(update) = replayed_token
            .and_then(|token| cluster.update_for_token(token))
            .filter(|update| update.scope == scope)
        {
            return Ok(UpdateCapabilityOutput {
                update: Some(update.to_update()),
            });
        }

        // One capability takes one update at a time: while its own update is running
        // the capability is in use, which is what `ResourceInUseException` reports,
        // worded by [`crate::state::update_in_progress`] for the update type this call
        // would have minted.
        if let Some(in_flight) = cluster.in_flight_update_in_scope(&scope) {
            return Err(UpdateCapabilityErrorModeled::ResourceInUseException(
                state::update_in_progress(UPDATE_TYPE_CAPABILITY, &input.cluster_name, in_flight),
            )
            .into());
        }

        // The state of the cluster and of the capability. A capability runs in a
        // running control plane, and it can only take a change once it has SETTLED: one
        // still being created, or being deleted, cannot.
        if cluster_status != STATUS_ACTIVE {
            return Err(invalid_parameter(format!(
                "Cluster {} is in {cluster_status} status; it must be {STATUS_ACTIVE} to update \
                 a capability.",
                input.cluster_name
            )));
        }
        if !CAPABILITY_STATUSES_AN_UPDATE_STARTS_FROM.contains(&capability_status.as_str()) {
            return Err(invalid_parameter(format!(
                "Capability {} is in {capability_status} status; it must be one of {} to be \
                 updated.",
                input.capability_name,
                CAPABILITY_STATUSES_AN_UPDATE_STARTS_FROM.join(", ")
            )));
        }

        // The change this update will carry, and the `params` that report it. "A
        // key-value map that contains the parameters associated with the update": one
        // entry per member of the request that asks for a change, so a caller reading
        // the update back sees what it asked for.
        let requested_argo_cd = input
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.argo_cd.as_ref());
        let requested_ack = input
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.ack.as_ref());
        let role_mappings = requested_argo_cd.and_then(|argo_cd| argo_cd.rbac_role_mappings.as_ref());
        let pending = PendingCapabilityUpdate {
            update_id: String::new(),
            role_arn: input.role_arn.clone(),
            add_or_update_role_mappings: role_mappings
                .and_then(|mappings| mappings.add_or_update_role_mappings.clone())
                .unwrap_or_default(),
            // A removal names the mapping by the ROLE it assigns, which is what
            // identifies one ("A mapping between an Argo CD role and IAM Identity
            // Center identities", `ArgoCdRoleMapping`), so the identities carried
            // beside it select nothing further.
            remove_role_mappings: role_mappings
                .and_then(|mappings| mappings.remove_role_mappings.clone())
                .unwrap_or_default()
                .into_iter()
                .map(|mapping| mapping.role)
                .collect(),
            network_access: requested_argo_cd.and_then(|argo_cd| {
                argo_cd
                    .network_access
                    .as_ref()
                    .map(|network| ArgoCdNetworkAccessConfigResponse {
                        vpce_ids: network.vpce_ids.clone(),
                    })
            }),
            enable_cross_namespace: requested_ack.and_then(|ack| ack.enable_cross_namespace),
            disabled_services: requested_ack.and_then(|ack| ack.disabled_services.clone()),
            delete_propagation_policy: input.delete_propagation_policy.clone(),
        };

        let mut params = Vec::new();
        if let Some(role_arn) = pending.role_arn.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_ROLE_ARN.to_string()),
                value: Some(role_arn.to_string()),
            });
        }
        if !pending.add_or_update_role_mappings.is_empty() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_ROLE_MAPPINGS_TO_ADD_OR_UPDATE.to_string()),
                value: Some(
                    pending
                        .add_or_update_role_mappings
                        .iter()
                        .map(|mapping| mapping.role.clone())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            });
        }
        if !pending.remove_role_mappings.is_empty() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_ROLE_MAPPINGS_TO_REMOVE.to_string()),
                value: Some(pending.remove_role_mappings.join(",")),
            });
        }
        if let Some(network_access) = pending.network_access.as_ref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_NETWORK_ACCESS.to_string()),
                value: Some(
                    network_access
                        .vpce_ids
                        .clone()
                        .unwrap_or_default()
                        .join(","),
                ),
            });
        }
        if let Some(enable_cross_namespace) = pending.enable_cross_namespace {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_ENABLE_CROSS_NAMESPACE.to_string()),
                value: Some(enable_cross_namespace.to_string()),
            });
        }
        if let Some(disabled_services) = pending.disabled_services.as_ref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_DISABLED_SERVICES.to_string()),
                value: Some(disabled_services.join(",")),
            });
        }
        // A request that asks for nothing would mint an update that changes nothing,
        // which is not a change this service records: the caller named no member to
        // update, and that is a bad parameter.
        if params.is_empty() {
            return Err(invalid_parameter(
                "the request must specify at least one of roleArn, configuration and \
                 deletePropagationPolicy to update"
                    .to_string(),
            ));
        }

        let completes_at = now + CAPABILITY_ACTIVATION_DELAY;
        let update_id = state::new_update_id(guard.next_update_sequence());
        let update = UpdateRecord {
            id: update_id.clone(),
            r#type: UPDATE_TYPE_CAPABILITY.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            params,
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(completes_at),
            // The change this update carries is the CAPABILITY's, not the cluster's:
            // it is parked on the capability record below and applied there when the
            // clock completes this update.
            pending_config: None,
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
            scope,
        };
        let answer = update.to_update();

        let server_error = |message: &str| {
            UpdateCapabilityError::from(UpdateCapabilityErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message.to_string()),
                    ..Default::default()
                },
            ))
        };

        // The update joins its cluster's history, which is where `DescribeUpdate` and
        // `ListUpdates` report it from.
        let cluster = guard.clusters.get_mut(&cluster_key).ok_or_else(|| {
            server_error("the cluster record disappeared while updating its capability")
        })?;
        cluster.record_update(update);

        // The capability is under the update until the clock completes it, and what it
        // reports changes only then — the update is asynchronous, so none of the
        // requested values is visible on the capability the instant it is requested.
        let record = guard.capabilities.get_mut(&capability_key).ok_or_else(|| {
            server_error("the capability record disappeared while updating it")
        })?;
        record.status = CAPABILITY_STATUS_UPDATING.to_string();
        record.active_at = Some(completes_at);
        record.pending_update = Some(PendingCapabilityUpdate {
            update_id,
            ..pending
        });

        Ok(UpdateCapabilityOutput {
            update: Some(answer),
        })
    }
}
