// SPDX-License-Identifier: CC0-1.0

//! @op DeprovisionIpamPoolCidr
//!
//! Takes one CIDR — or every CIDR, for a request that names none — back out of an IPAM pool,
//! so the pool-CIDR dependency `DeleteIpamPool` is refused for is cleared and no later reader
//! of the store reports a range the pool no longer holds.
//!
//! A range the pool holds no provisioned CIDR for is nothing this call can take back, so it is
//! refused for the STATE of the resource the request names — EC2's `IncorrectState`, naming the
//! range the request asked about — rather than answered as a deprovision that did nothing.
//!
//! The range is left in the transient `pending-deprovision` state the `IpamPoolCidrState`
//! catalog carries and the clock owes it the removal one
//! [`crate::state::TRANSITION_DELAY`] later, through
//! [`crate::state::ComAmazonawsEc2Amazonec2State::deprovision_ipam_pool_cidr`] — the one
//! writer that takes a range back, so a cascade leaves one in exactly the state this call
//! does.
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
//! # Modeled contract for `DeprovisionIpamPoolCidr`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Deprovision a CIDR provisioned from an IPAM pool. If you deprovision a CIDR from a pool
//! that has a source pool, the CIDR is recycled back into the source pool. For more
//! information, see Deprovision pool CIDRs in the Amazon VPC IPAM User Guide.
//!
//! ## Input
//!
//! * `Cidr` (String) — The CIDR which you want to deprovision from the pool.
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `IpamPoolId` (IpamPoolId) **required** — The ID of the pool that has the CIDR you want
//!   to deprovision.
//!
//! ## Output
//!
//! * `IpamPoolCidr` (IpamPoolCidr) — The deprovisioned pool CIDR.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeprovisionIpamPoolCidrError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeprovisionIpamPoolCidrError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DeprovisionIpamPoolCidr")]
#[derive(Debug)]
pub enum DeprovisionIpamPoolCidrErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::deprovision_ipam_pool_cidr::DeprovisionIpamPoolCidrError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeprovisionIpamPoolCidrError = crate::scaffolded::Scaffolded<DeprovisionIpamPoolCidrErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeprovisionIpamPoolCidrErrorModeled> for DeprovisionIpamPoolCidrError {
    fn from(modeled: DeprovisionIpamPoolCidrErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeprovisionIpamPoolCidr` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeprovisionIpamPoolCidr<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeprovisionIpamPoolCidr<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeprovisionIpamPoolCidr<Eng>
{
    type Input = DeprovisionIpamPoolCidrInput;
    type Output = DeprovisionIpamPoolCidrOutput;
    type Error = DeprovisionIpamPoolCidrError;

    const NAME: &'static str = "DeprovisionIpamPoolCidr";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEPROVISION_IPAM_POOL_CIDR_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEPROVISION_IPAM_POOL_CIDR_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; this operation reads none of it — the pool is
        // named in the request and carries its own Region. The store is reached through the
        // engine held on `self`.
        let _ = ctx;

        // The request's own FORM first, before anything is resolved and before the DryRun arm
        // below: "not an IPAM pool id at all" is a malformed request rather than a pool that
        // does not exist.
        let pool_id = input.ipam_pool_id.as_str();
        state::require_well_formed_id(
            state::IPAM_POOL_ID_PREFIX,
            pool_id,
            state::INVALID_IPAM_POOL_ID_MALFORMED,
        )?;

        // "A check for whether you have the required permissions for the action without
        // actually making the request" (`$DryRun`): a question about the CALLER, answered once
        // the request's own form has been checked and before anything is resolved or written.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        let state_handle = state::require_service_state(&*self.engine)?;
        let mut store = state_handle.lock();
        // The clock first, the way every other state-touching handler in this crate starts, so
        // every question below is asked of the state the store is ACTUALLY in.
        store.materialize_due_transitions();

        // "The ID of the pool that has the CIDR you want to deprovision" (`$IpamPoolId`): an
        // id this account holds no pool for is `.NotFound`.
        if !store.ipam_pools.contains_key(pool_id) {
            return Err(state::id_not_found(
                state::INVALID_IPAM_POOL_ID_NOT_FOUND,
                "IPAM pool ID",
                pool_id,
            ));
        }

        // "The CIDR which you want to deprovision from the pool" (`$Cidr`) names ONE range of
        // the pool; a request that names none asks for every range the pool holds, which is
        // what the IPAM User Guide's "deprovision all CIDRs from a pool" step does. Either
        // way the ranges the pool holds are read through the ONE reader of that question, so
        // a range a request names is the same range a delete is refused for.
        let requested = input.cidr.as_deref().filter(|cidr| !cidr.is_empty());
        let ranges: Vec<String> = match requested {
            Some(cidr) => {
                // The range is matched in the canonical form the store keys it by, so a range
                // spelled with host bits set still names the range it is part of.
                let canonical = state::canonical_cidr_block(cidr).unwrap_or_else(|| cidr.to_string());
                let held = store
                    .ipam_pool_provisioned_cidrs(pool_id)
                    .into_iter()
                    .find(|held| *held == canonical);
                vec![held.ok_or_else(|| pool_cidr_does_not_exist(cidr))?]
            }
            None => store.ipam_pool_provisioned_cidrs(pool_id),
        };
        if ranges.is_empty() {
            return Err(pool_holds_no_cidr(pool_id));
        }

        // "If you deprovision a CIDR from a pool that has a source pool, the CIDR is recycled
        // back into the source pool" (`DeprovisionIpamPoolCidr`): the range stops being one of
        // this pool's, which is exactly what the source pool having it back means — the space
        // is free for the source pool to hand out again.
        //
        // Each range is left in the TRANSIENT `pending-deprovision` state with the clock owing
        // its removal, through the one writer of that store, so a caller reading straight back
        // still sees the deprovision under way.
        let mut deprovisioned: Vec<IpamPoolCidr> = Vec::with_capacity(ranges.len());
        for cidr in &ranges {
            if let Some(view) = store.deprovision_ipam_pool_cidr(pool_id, cidr) {
                deprovisioned.push(view);
            }
        }

        // "The deprovisioned pool CIDR" (`$IpamPoolCidr`) is ONE range, so a request that
        // named one answers that range; one that named none took every range the pool held,
        // and answers the first of them in the store's own order.
        let Some(view) = deprovisioned.into_iter().next() else {
            // A range resolved above is one this pool holds, and the store is locked
            // throughout, so this cannot be reached; kept so a deprovision that wrote nothing
            // can never read as a 200 over a range the pool still holds.
            return Err(state::internal_error(
                "the IPAM pool CIDR resolved a moment ago could not be deprovisioned",
            ));
        };
        Ok(DeprovisionIpamPoolCidrOutput {
            ipam_pool_cidr: Some(view),
        })
    }
}

/// Refuse a deprovision of a range the pool holds no provisioned CIDR for: HTTP 400
/// `IncorrectState`.
///
/// The pool itself resolves, so there is no id to report as not found; what is wrong is the
/// STATE of the resource the request names — the pool holds nothing to take back — which is
/// the class EC2 answers `IncorrectState` for ("The resource is not in the correct state for
/// the request"), not the parameter-value class a value that is no range at all would be. The
/// sentence is EC2's own "the specified action is not valid" wording for IPAM, which
/// [`crate::ops::delete_ipam_pool`] and [`crate::ops::create_ipam_pool`] carry for their own
/// refusals of this family, followed by what is not valid about it.
///
/// The range is named as the REQUEST spelled it rather than in the canonical form it was
/// looked up by — nothing is stored for it, so the caller's own bytes are the only name it has
/// — bounded by the ceiling every refusal of this crate echoes a CIDR block to
/// ([`crate::ops::delete_coip_cidr::echoed_cidr`]).
fn pool_cidr_does_not_exist(cidr: &str) -> DeprovisionIpamPoolCidrError {
    crate::state::incorrect_state(format!(
        "The specified action is not valid.The pool CIDR {cidr} does not exist.",
        cidr = crate::ops::delete_coip_cidr::echoed_cidr(cidr),
    ))
}

/// Refuse a deprovision of EVERY range of a pool that holds none: HTTP 400 `IncorrectState`,
/// the same class as [`pool_cidr_does_not_exist`] for the same reason — a pool with nothing
/// provisioned to it has nothing to take back.
///
/// The sentence is the pool-scoped form of that wording, "The specified action is not valid
/// for `<pool>`." followed by what is wrong with the pool, which is how
/// [`crate::ops::delete_ipam_pool`] words the opposite refusal over the same question ("There
/// are one or more pool cidrs provisioned in the pool.") — the one reader of what a pool
/// holds answers both.
fn pool_holds_no_cidr(pool_id: &str) -> DeprovisionIpamPoolCidrError {
    crate::state::incorrect_state(format!(
        "The specified action is not valid for {pool_id}. There are no pool cidrs provisioned \
         in the pool."
    ))
}
