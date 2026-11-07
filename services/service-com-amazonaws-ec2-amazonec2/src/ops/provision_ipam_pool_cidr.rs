// SPDX-License-Identifier: CC0-1.0

//! @op ProvisionIpamPoolCidr
//!
//! Provisions one CIDR to an IPAM pool and stores it, so every later reader of the store —
//! `GetIpamPoolCidrs`, and the pool-CIDR dependency `DeleteIpamPool` is refused for — reports
//! the same range this call answered.
//!
//! What the service owns is derived here rather than taken from the request: the
//! `IpamPoolCidrId` and the `pending-provision` state the range starts in, which
//! [`crate::state::ComAmazonawsEc2Amazonec2State::provision_ipam_pool_cidr`] mints — the clock
//! owes it `provisioned` one [`crate::state::TRANSITION_DELAY`] later — and the netmask length,
//! which is the range's own.
//!
//! The pool is the whole of the resolution: a pool id this account holds none of is refused
//! with `InvalidIpamPoolId.NotFound`, the range is read in the family that pool holds, and a
//! range meeting one the pool already holds, or outside the space a source pool hands it, is
//! refused before anything is stored.
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
//! # Modeled contract for `ProvisionIpamPoolCidr`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Provision a CIDR to an IPAM pool. You can use this action to provision new CIDRs to a
//! top-level pool or to transfer a CIDR from a top-level pool to a pool within it. For more
//! information, see Provision CIDRs to pools in the Amazon VPC IPAM User Guide.
//!
//! ## Input
//!
//! * `Cidr` (String) — The CIDR you want to assign to the IPAM pool. Either "NetmaskLength"
//!   or "Cidr" is required. This value will be null if you specify "NetmaskLength" and will
//!   be filled in during the provisioning process.
//! * `CidrAuthorizationContext` (IpamCidrAuthorizationContext) — A signed document that
//!   proves that you are authorized to bring a specified IP address range to Amazon using
//!   BYOIP. This option only applies to IPv4 and IPv6 pools in the public scope.
//! * `ClientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier that you
//!   provide to ensure the idempotency of the request. For more information, see Ensuring
//!   idempotency.
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `IpamExternalResourceVerificationTokenId` (IpamExternalResourceVerificationTokenId) —
//!   Verification token ID. This option only applies to IPv4 and IPv6 pools in the public
//!   scope.
//! * `IpamPoolId` (IpamPoolId) **required** — The ID of the IPAM pool to which you want to
//!   assign a CIDR.
//! * `NetmaskLength` (Integer) — The netmask length of the CIDR you'd like to provision to a
//!   pool. Can be used for provisioning Amazon-provided IPv6 CIDRs to top-level pools and for
//!   provisioning CIDRs to pools with source pools. Cannot be used to provision BYOIP CIDRs
//!   to top-level pools. Either "NetmaskLength" or "Cidr" is required.
//! * `VerificationMethod` (VerificationMethod, one of: dns-token | remarks-x509) — The method
//!   for verifying control of a public IP address range. Defaults to remarks-x509 if not
//!   specified. This option only applies to IPv4 and IPv6 pools in the public scope.
//!
//! ## Output
//!
//! * `IpamPoolCidr` (IpamPoolCidr) — Information about the provisioned CIDR.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ProvisionIpamPoolCidrError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ProvisionIpamPoolCidrError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#ProvisionIpamPoolCidr")]
#[derive(Debug)]
pub enum ProvisionIpamPoolCidrErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::provision_ipam_pool_cidr::ProvisionIpamPoolCidrError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ProvisionIpamPoolCidrError = crate::scaffolded::Scaffolded<ProvisionIpamPoolCidrErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ProvisionIpamPoolCidrErrorModeled> for ProvisionIpamPoolCidrError {
    fn from(modeled: ProvisionIpamPoolCidrErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ProvisionIpamPoolCidr` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ProvisionIpamPoolCidr<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ProvisionIpamPoolCidr<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ProvisionIpamPoolCidr<Eng>
{
    type Input = ProvisionIpamPoolCidrInput;
    type Output = ProvisionIpamPoolCidrOutput;
    type Error = ProvisionIpamPoolCidrError;

    const NAME: &'static str = "ProvisionIpamPoolCidr";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PROVISION_IPAM_POOL_CIDR_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PROVISION_IPAM_POOL_CIDR_OUTPUT_SCHEMA;
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
        // does not exist, which is the order every resource of this crate asks the two
        // questions in.
        let pool_id = input.ipam_pool_id.as_str();
        state::require_well_formed_id(
            state::IPAM_POOL_ID_PREFIX,
            pool_id,
            state::INVALID_IPAM_POOL_ID_MALFORMED,
        )?;

        // "Either 'NetmaskLength' or 'Cidr' is required" (`$Cidr` / `$NetmaskLength`): a
        // request that names neither asks for no range at all, and one that names both says
        // two different things about the size of the range it wants.
        let requested_cidr = input.cidr.as_deref().filter(|cidr| !cidr.is_empty());
        match (requested_cidr, input.netmask_length) {
            (None, None) => {
                return Err(state::missing_parameter(
                    "Either 'cidr' or 'netmaskLength' must be provided",
                ));
            }
            (Some(_), Some(_)) => {
                return Err(state::invalid_parameter_combination(
                    "Only one of 'cidr' or 'netmaskLength' may be provided",
                ));
            }
            _ => {}
        }

        // "A unique, case-sensitive identifier that you provide to ensure the idempotency of
        // the request" (`$ClientToken`): bounded the way every other caller-chosen token of
        // this crate is, since nothing reclaims the bytes it would retain.
        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > state::MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(state::invalid_parameter_value(format!(
                "clientToken accepts at most {max} characters",
                max = state::MAX_CLIENT_TOKEN_CHARS,
            )));
        }

        // "A check for whether you have the required permissions for the action WITHOUT
        // ACTUALLY MAKING THE REQUEST" (`$DryRun`): a question about the CALLER, answered once
        // the request's own form has been checked and before anything is resolved or written.
        // Every caller of this emulator is permitted.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        let state_handle = state::require_service_state(&*self.engine)?;
        let mut store = state_handle.lock();
        // The clock first, the way every other state-touching handler in this crate starts: a
        // pool whose `create-complete` move has fallen due is already in that state, and so is
        // a CIDR whose own provision has settled, so every question below is asked of the
        // state the store is ACTUALLY in.
        store.materialize_due_transitions();

        // "The ID of the IPAM pool to which you want to assign a CIDR" (`$IpamPoolId`): an id
        // this account holds no pool for is `.NotFound`, and a pool on its way out is not one
        // a CIDR can be added to.
        let pool = store.ipam_pools.get(pool_id).cloned().ok_or_else(|| {
            state::id_not_found(
                state::INVALID_IPAM_POOL_ID_NOT_FOUND,
                "IPAM pool ID",
                pool_id,
            )
        })?;
        if state::ipam_pool_is_deleting(&pool) {
            return Err(state::incorrect_state_of(
                "IPAM pool",
                pool_id,
                pool.state.as_deref().unwrap_or_default(),
            ));
        }

        let family = pool.address_family.as_deref().unwrap_or_default();
        // "The netmask length of the CIDR you'd like to provision to a pool. Can be used for
        // provisioning Amazon-provided IPv6 CIDRs to top-level pools and for provisioning
        // CIDRs to pools with source pools. CANNOT BE USED TO PROVISION BYOIP CIDRS TO
        // TOP-LEVEL POOLS" (`$NetmaskLength`): a request that names a SIZE rather than a range
        // asks the service to choose the range, which it can only do out of space it holds —
        // the source pool's for a pool within one, and Amazon's own IPv6 space for a top-level
        // IPv6 pool. A top-level pool of a family with no such space has none to choose from.
        let requested_cidr: String = match (requested_cidr, input.netmask_length) {
            (Some(cidr), _) => cidr.to_string(),
            (None, Some(netmask_length)) => {
                let netmask_length = u32::try_from(netmask_length).unwrap_or(u32::MAX);
                match pool.source_ipam_pool_id.as_deref() {
                    Some(source_pool_id) => {
                        // The space the source pool holds, less every range already given OUT
                        // of it, is what this pool may be given: the first free block of the
                        // requested size, so two pools drawing on one source pool cannot be
                        // given overlapping space. What is "taken" is the space handed to the
                        // pools FED BY this source pool — the source pool's own ranges are the
                        // space being handed out, and a pool of another source pool's space
                        // cannot meet it — so the choice is a function of this source pool's
                        // own allocations.
                        let space = store.ipam_pool_provisioned_cidrs(source_pool_id);
                        let fed_by_source: ::std::collections::BTreeSet<&str> = store
                            .ipam_pools
                            .iter()
                            .filter(|(_, held)| {
                                held.source_ipam_pool_id.as_deref() == Some(source_pool_id)
                            })
                            .map(|(fed_pool_id, _)| fed_pool_id.as_str())
                            .collect();
                        let taken: Vec<String> = store
                            .ipam_pool_cidrs
                            .values()
                            .filter(|record| {
                                fed_by_source.contains(record.ipam_pool_id.as_str())
                                    && state::ipam_pool_cidr_holds_space(
                                        record.view.state.as_deref(),
                                    )
                            })
                            .filter_map(|record| record.view.cidr.clone())
                            .collect();
                        // The cost of the search below is the product of the space, the
                        // candidates inside it and the ranges already given out, so it is
                        // charged to ONE budget — the bound the per-range figure alone cannot
                        // give.
                        let mut budget =
                            state::MatchBudget::new(state::MAX_FILTER_MATCH_STEPS);
                        first_free_block(&space, netmask_length, &taken, &mut budget)
                            .map_err(|()| search_budget_spent())?
                            .ok_or_else(|| {
                                state::invalid_parameter_value(format!(
                                    "Invalid value '{netmask_length}' for netmaskLength. The \
                                     source IPAM pool '{source_pool_id}' has no free CIDR of \
                                     that size"
                                ))
                            })?
                    }
                    None if family == state::ADDRESS_FAMILY_IPV6 => {
                        // "An Amazon-provided IPv6 CIDR block" comes out of the space Amazon
                        // advertises them from, minted by the one writer of that space
                        // ([`crate::state::ComAmazonawsEc2Amazonec2State::mint_amazon_ipv6_cidr`]).
                        // The SIZE the request asks for is checked against that block's own
                        // before anything is minted, so a request this call goes on to refuse
                        // consumes no range: "/56" is what Amazon provides, and a block of it
                        // is what this pool is given.
                        if !(AMAZON_PROVIDED_IPV6_PREFIX_LENGTH..=IPV6_ADDRESS_BITS)
                            .contains(&netmask_length)
                        {
                            return Err(state::invalid_parameter_value(format!(
                                "Invalid value '{netmask_length}' for netmaskLength. An \
                                 Amazon-provided IPv6 CIDR is a \
                                 /{AMAZON_PROVIDED_IPV6_PREFIX_LENGTH} block, so no range of \
                                 that size can be taken from it"
                            )));
                        }
                        let amazon = store.mint_amazon_ipv6_cidr();
                        let space = ::std::slice::from_ref(&amazon);
                        let mut budget =
                            state::MatchBudget::new(state::MAX_FILTER_MATCH_STEPS);
                        first_free_block(space, netmask_length, &[], &mut budget)
                            .map_err(|()| search_budget_spent())?
                            .ok_or_else(|| {
                                state::internal_error(format!(
                                    "the Amazon-provided range '{amazon}' holds no \
                                     /{netmask_length} block"
                                ))
                            })?
                    }
                    None => {
                        return Err(state::invalid_parameter_combination(
                            "netmaskLength cannot be used to provision BYOIP CIDRs to \
                             top-level pools; name the cidr instead",
                        ));
                    }
                }
            }
            (None, None) => {
                // Refused above, where the request's own form is read.
                return Err(state::missing_parameter(
                    "Either 'cidr' or 'netmaskLength' must be provided",
                ));
            }
        };
        let requested_cidr = requested_cidr.as_str();

        // "A CIDR is a representation of an IP address and its associated network mask"
        // (`IpamPoolCidr$Cidr`): the range is read in the family the POOL holds — "you must
        // choose either IPv4 or IPv6 protocol for a pool"
        // (`CreateIpamPoolRequest$AddressFamily`) — so an IPv6 range cannot be provisioned to
        // an IPv4 pool, and the stored form is the canonical one, with its host bits cleared,
        // so one range cannot be spelled two ways in one pool.
        let canonical = match family {
            state::ADDRESS_FAMILY_IPV6 => state::canonical_ipv6_cidr(requested_cidr),
            _ => state::canonical_ipv4_cidr(requested_cidr),
        }
        .map_err(|_| {
            state::invalid_parameter_value(format!(
                "Invalid value '{requested_cidr}' for cidr. It must be an {family} CIDR block"
            ))
        })?;

        // A range the pool ALREADY holds a record for is not one it can be given again,
        // whatever state that record is in: a pool holds at most one record per range — a
        // second would make "which record is this range's" ambiguous — so a range whose
        // deprovision is still running has to finish leaving before it can come back.
        if let Some(held) = store.ipam_pool_cidr(pool_id, &canonical) {
            return Err(state::invalid_parameter_value(format!(
                "Invalid value '{canonical}' for cidr. The IPAM pool '{pool_id}' already holds \
                 that CIDR, in state '{state}'",
                state = held.view.state.as_deref().unwrap_or_default(),
            )));
        }

        // "The CIDR you want to assign to the IPAM pool" has to be space the pool can hand
        // out, so a range meeting one the pool already holds would hand the same address out
        // twice — through the one reader of what a pool holds
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::ipam_pool_provisioned_cidrs`]), so
        // a BYOIP range moved into the pool counts exactly as a provisioned one does.
        if let Some(held) = store
            .ipam_pool_provisioned_cidrs(pool_id)
            .into_iter()
            .find(|held| state::cidr_blocks_overlap(held, &canonical))
        {
            return Err(state::invalid_parameter_value(format!(
                "Invalid value '{canonical}' for cidr. It overlaps with the CIDR '{held}' \
                 already provisioned to the IPAM pool '{pool_id}'"
            )));
        }

        // "You can use this action to provision new CIDRs to a top-level pool or to TRANSFER
        // A CIDR FROM A TOP-LEVEL POOL TO A POOL WITHIN IT" (`ProvisionIpamPoolCidr`): a pool
        // created within another draws its space from that pool, so a range outside
        // everything the source pool holds is space this pool could never hand out.
        if let Some(source_pool_id) = pool.source_ipam_pool_id.as_deref() {
            let from_source = store
                .ipam_pool_provisioned_cidrs(source_pool_id)
                .into_iter()
                .any(|source| state::cidr_block_contains_block(&source, &canonical));
            if !from_source {
                return Err(state::invalid_parameter_value(format!(
                    "Invalid value '{canonical}' for cidr. It is not within a CIDR \
                     provisioned to the source IPAM pool '{source_pool_id}'"
                )));
            }
        }

        // The netmask length the range itself names, which is what the pool reports for it
        // ("the netmask length of the CIDR you'd like to provision to a pool") — derived from
        // the range rather than taken from the request, which named none.
        let netmask_length = state::cidr_block_prefix_length(&canonical)
            .and_then(|length| i32::try_from(length).ok());

        // Stored through the one writer of this store, which mints the id and the
        // `pending-provision` state and records the move to `provisioned` the clock owes it;
        // `None` means the store is at a ceiling and NOTHING was stored.
        store
            .provision_ipam_pool_cidr(pool_id, &canonical, netmask_length)
            .map(|ipam_pool_cidr| ProvisionIpamPoolCidrOutput {
                ipam_pool_cidr: Some(ipam_pool_cidr),
            })
            .ok_or_else(|| {
                state::resource_limit_exceeded(format!(
                    "This emulator holds {max} IPAM pool CIDRs, which is as many as it stores",
                    max = state::MAX_STORED_RECORDS_PER_STORE,
                ))
            })
    }
}

/// The first block of `netmask_length` bits inside `space` that meets none of `taken`, or
/// `None` when the space holds no such block — and `Err` when finding out would cost more than
/// `budget` allows.
///
/// What a `ProvisionIpamPoolCidr` that names a SIZE rather than a range is given: IPAM chooses
/// the range out of space it holds, so the choice is the first free block of the asked-for size
/// in the order the space is held — which makes the choice a function of the store rather than
/// of the call, so two calls never give out overlapping space.
///
/// Each candidate comes from [`crate::state::nth_cidr_block_in`], the crate's one reading of a
/// CIDR's numeric bounds, so one walk serves IPv4 and IPv6 and a block chosen here is spelled
/// as every other block of this crate is.
///
/// The cost of the walk is the PRODUCT of the ranges in `space`, the candidates inside each one
/// (which the request's netmask length decides) and the ranges in `taken` — all three grown by
/// what the store holds — so ONE [`crate::state::MatchBudget`] is charged a step per candidate
/// AND a step per comparison against a held range, exactly as the filter matching of
/// `crate::ops::get_ipam_pool_cidrs` charges its own product. A spent budget is an answer the
/// request cannot be served rather than a search that runs on under the state lock.
fn first_free_block(
    space: &[String],
    netmask_length: u32,
    taken: &[String],
    budget: &mut crate::state::MatchBudget,
) -> Result<Option<String>, ()> {
    for range in space {
        for index in 0..MAX_CANDIDATE_BLOCKS {
            if !budget.spend() {
                return Err(());
            }
            let Some(candidate) = crate::state::nth_cidr_block_in(range, netmask_length, index)
            else {
                // Past the end of this range — or a size it cannot be cut into at all, which
                // the first index already answers.
                break;
            };
            let mut free = true;
            for held in taken {
                if !budget.spend() {
                    return Err(());
                }
                if crate::state::cidr_blocks_overlap(held, &candidate) {
                    free = false;
                    break;
                }
            }
            if free {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

/// How many candidate blocks [`first_free_block`] may try inside ONE range.
///
/// A bound on the WORK one request can ask for, not an AWS quota: without it a request naming a
/// long netmask length inside a short range would walk billions of candidates under the state
/// lock. The budget above bounds the walk ACROSS ranges, which this per-range figure alone
/// cannot.
const MAX_CANDIDATE_BLOCKS: u128 = 4_096;

/// The prefix length of an Amazon-provided IPv6 CIDR block: "an Amazon-provided IPv6 CIDR block
/// with a /56 prefix length" (`CreateVpcRequest$AmazonProvidedIpv6CidrBlock`), which is the
/// block [`crate::state::ComAmazonawsEc2Amazonec2State::mint_amazon_ipv6_cidr`] mints — so a
/// request asking for a LARGER range than that is asking for space Amazon does not provide.
const AMAZON_PROVIDED_IPV6_PREFIX_LENGTH: u32 = 56;

/// How many bits an IPv6 address has, which is the longest netmask an IPv6 block can carry.
const IPV6_ADDRESS_BITS: u32 = 128;

/// Refuse a request whose search for a free block would cost more than one call may: HTTP 400
/// `RequestLimitExceeded`, the answer `crate::ops::get_ipam_pool_cidrs` gives a request whose
/// filtering does not fit its own budget.
///
/// An INCOMPLETE search is not something to report as a result: the request asked for more work
/// than one call will do, and is told so rather than given a range the search had not finished
/// clearing — or left running under the state lock.
fn search_budget_spent() -> ProvisionIpamPoolCidrError {
    crate::scaffolded::Scaffolded::request_limit_exceeded(format!(
        "Choosing a CIDR for this pool would take more than \
         {max} comparisons; provision the cidr itself instead",
        max = crate::state::MAX_FILTER_MATCH_STEPS,
    ))
}
