// SPDX-License-Identifier: CC0-1.0

//! @op DeleteIpam
//!
//! Deletes one IPAM, so no later reader of the store reports an IPAM — or a scope, a pool or a
//! provisioned CIDR inside it — the account no longer holds: `DescribeIpams` for the id then
//! answers `InvalidIpamId.NotFound`.
//!
//! "Deleting an IPAM removes all monitored data associated with the IPAM" (`DeleteIpam`), so
//! what the store holds INSIDE one has to be accounted for first:
//! [`crate::state::ComAmazonawsEc2Amazonec2State::ipam_pool_ids_in_ipam_scopes`] is what a pool
//! still in one of its scopes is read through, and `Cascade` is the one way to delete those
//! pools along with the IPAM — "you cannot delete the IPAM with this option if there is a pool
//! in your public scope" (`$Cascade`).
//!
//! The IPAM is answered in the transient `delete-in-progress` state the `IpamState` catalog
//! carries, with the clock owing its REMOVAL one [`crate::state::TRANSITION_DELAY`] later
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::begin_ipam_deletion`]) — recorded rather
//! than slept for, so a caller that reads straight back still sees the delete under way and a
//! later one finds the IPAM gone.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store is
//! reached through [`crate::state::require_service_state`], the way every handler beside this
//! one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteIpam`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Delete an IPAM. Deleting an IPAM removes all monitored data associated with the IPAM
//! including the historical data for CIDRs. For more information, see Delete an IPAM in the
//! Amazon VPC IPAM User Guide.
//!
//! ## Input
//!
//! * `Cascade` (Boolean) — Enables you to quickly delete an IPAM, private scopes, pools in
//!   private scopes, and any allocations in the pools in private scopes. You cannot delete
//!   the IPAM with this option if there is a pool in your public scope. If you use this
//!   option, IPAM does the following: Deallocates any CIDRs allocated to VPC resources (such
//!   as VPCs) in pools in private scopes. No VPC resources are deleted as a result of
//!   enabling this option. The CIDR associated with the resource will no longer be allocated
//!   from an IPAM pool, but the CIDR itself will remain unchanged. Deprovisions all IPv4
//!   CIDRs provisioned to IPAM pools in private scopes. Deletes all IPAM pools in private
//!   scopes. Deletes all non-default private …
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `IpamId` (IpamId) **required** — The ID of the IPAM to delete.
//!
//! ## Output
//!
//! * `Ipam` (Ipam) — Information about the results of the deletion.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteIpamError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteIpamError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DeleteIpam")]
#[derive(Debug)]
pub enum DeleteIpamErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_ipam::DeleteIpamError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteIpamError = crate::scaffolded::Scaffolded<DeleteIpamErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteIpamErrorModeled> for DeleteIpamError {
    fn from(modeled: DeleteIpamErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteIpam` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteIpam<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteIpam<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteIpam<Eng>
{
    type Input = DeleteIpamInput;
    type Output = DeleteIpamOutput;
    type Error = DeleteIpamError;

    const NAME: &'static str = "DeleteIpam";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IPAM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IPAM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; this operation reads none of it — an IPAM is
        // named by id and reports its own home Region (`Ipam$IpamRegion`). The store is
        // reached through the engine held on `self`.
        let _ = ctx;

        // The request's own FORM first, before anything is resolved and before the DryRun arm
        // below: "not an IPAM id at all" is a malformed request rather than an IPAM that does
        // not exist, which is the order every resource of this crate asks the two questions
        // in.
        let ipam_id = input.ipam_id.as_str();
        state::require_well_formed_id(
            state::IPAM_ID_PREFIX,
            ipam_id,
            state::INVALID_IPAM_ID_MALFORMED,
        )?;

        // "A check for whether you have the required permissions for the action without
        // actually making the request" (`$DryRun`): a question about the CALLER, answered once
        // the request's own form has been checked and before anything is resolved or written;
        // every caller of this emulator is permitted.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        let state_handle = state::require_service_state(&*self.engine)?;
        let mut store = state_handle.lock();
        // The clock first, the way every other state-touching handler in this crate starts: an
        // IPAM whose `create-complete` move has fallen due is already in that state, and one
        // whose own deletion has already settled is GONE, so every question below is asked of
        // the state the store is ACTUALLY in.
        store.materialize_due_transitions();

        // "The ID of the IPAM to delete" (`$IpamId`): an id this account holds no IPAM for is
        // `.NotFound`, in this resource's own wording.
        let ipam = store
            .ipams
            .get(ipam_id)
            .cloned()
            .ok_or_else(|| state::invalid_ipam_id_not_found(ipam_id))?;
        // An IPAM a delete has already been asked for has nothing left to delete, so a second
        // delete is refused rather than restarting the clock on it — in EC2's own wording for
        // a resource refused SOLELY for the state it is in.
        if state::ipam_is_deleting(&ipam) {
            return Err(state::incorrect_state_of(
                "IPAM",
                ipam_id,
                ipam.state.as_deref().unwrap_or_default(),
            ));
        }

        // The pools the IPAM still holds, by the scope they are in: what decides whether this
        // delete is admissible at all.
        let public_pools = store.ipam_pool_ids_in_ipam_scopes(ipam_id, state::IPAM_SCOPE_TYPE_PUBLIC);
        let private_pools =
            store.ipam_pool_ids_in_ipam_scopes(ipam_id, state::IPAM_SCOPE_TYPE_PRIVATE);

        if input.cascade.unwrap_or(false) {
            // "You cannot delete the IPAM with this option if there is a pool in your public
            // scope" (`$Cascade`): the option covers the private scopes only, so a public-scope
            // pool is one the caller still has to take away itself.
            if !public_pools.is_empty() {
                return Err(state::dependency_violation(format!(
                    "The specified action is not valid for {ipam_id}. There are one or more \
                     pools in the public scope of the IPAM."
                )));
            }
            // "Deprovisions all IPv4 CIDRs provisioned to IPAM pools in private scopes.
            // Deletes all IPAM pools in private scopes" (`$Cascade`): each pool goes on
            // exactly the terms `DeleteIpamPool`'s own cascade puts it on — deepest first, so
            // no pool is left inside a pool that is already going — through the same two
            // writers, so a pool deleted this way and one deleted directly end up in the same
            // state.
            for pool_id in &private_pools {
                for inner_pool_id in store.ipam_pools_within(pool_id) {
                    store.deprovision_ipam_pool_cidrs(&inner_pool_id);
                    store.begin_ipam_pool_deletion(&inner_pool_id);
                }
                store.deprovision_ipam_pool_cidrs(pool_id);
                store.begin_ipam_pool_deletion(pool_id);
            }
        } else if !public_pools.is_empty() || !private_pools.is_empty() {
            // A pool is a resource within the IPAM, and the scopes it lives in exist for as
            // long as the IPAM does, so an IPAM that still holds one cannot be deleted —
            // `Cascade` is the one way to delete them along with it. EC2's own code for a
            // delete the resource's dependencies forbid, worded the way the IPAM family words
            // that refusal: the action, the resource it is not valid for, and why.
            return Err(state::dependency_violation(format!(
                "The specified action is not valid for {ipam_id}. There are one or more pools \
                 in the ipam."
            )));
        }

        // "Information about the results of the deletion" (`$Ipam`): the IPAM as it was STORED
        // by the move this delete started, so the caller sees what a later reader will see
        // rather than a value built here.
        let view = store.begin_ipam_deletion(ipam_id).ok_or_else(|| {
            state::internal_error(format!(
                "the IPAM '{ipam_id}' resolved a moment ago is not in the store any more"
            ))
        })?;

        Ok(DeleteIpamOutput { ipam: Some(view) })
    }
}
