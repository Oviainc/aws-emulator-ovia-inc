// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateSecurityGroupVpc
//! Takes down the association that made one of the account's security groups usable in a VPC
//! it was not created in, so the group is no longer usable by network interfaces and
//! instances in that VPC.
//!
//! The association is the (`GroupId`, `VpcId`) PAIR and nothing else, so this operation
//! addresses it by that pair in
//! [`crate::state::ComAmazonawsEc2Amazonec2State::security_group_vpc_associations`] — the
//! store `AssociateSecurityGroupVpc` writes and `DescribeSecurityGroupVpcAssociations`
//! reports — and answers it through the one view all three report an association with. It is
//! answered in the non-terminal `disassociating` state, with the clock — not a second call —
//! owing it `disassociated`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateSecurityGroupVpc`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Disassociates a security group from a VPC. You cannot disassociate the security group if
//! any Elastic network interfaces in the associated VPC are still associated with the
//! security group. Note that the disassociation is asynchronous and you can check the
//! status of the request with DescribeSecurityGroupVpcAssociations.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `GroupId` (DisassociateSecurityGroupVpcSecurityGroupId) **required** — A security group
//!   ID.
//! * `VpcId` (String) **required** — A VPC ID.
//!
//! ## Output
//!
//! * `State` (SecurityGroupVpcAssociationState, one of: associated | associating |
//!   association-failed | disassociated | disassociating | disassociation-failed) — The state
//!   of the disassociation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DisassociateSecurityGroupVpcError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateSecurityGroupVpcError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DisassociateSecurityGroupVpc")]
#[derive(Debug)]
pub enum DisassociateSecurityGroupVpcErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disassociate_security_group_vpc::DisassociateSecurityGroupVpcError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisassociateSecurityGroupVpcError = crate::scaffolded::Scaffolded<DisassociateSecurityGroupVpcErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisassociateSecurityGroupVpcErrorModeled> for DisassociateSecurityGroupVpcError {
    fn from(modeled: DisassociateSecurityGroupVpcErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateSecurityGroupVpc` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisassociateSecurityGroupVpc<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateSecurityGroupVpc<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateSecurityGroupVpc<Eng>
{
    type Input = DisassociateSecurityGroupVpcInput;
    type Output = DisassociateSecurityGroupVpcOutput;
    type Error = DisassociateSecurityGroupVpcError;

    const NAME: &'static str = "DisassociateSecurityGroupVpc";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_SECURITY_GROUP_VPC_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_SECURITY_GROUP_VPC_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; every fact this disassociation reports comes
        // from the request and from the store the engine on `self` holds.
        let _ = ctx;

        // Both ids are `@required` but modelled `clientOptional`, so a request that left
        // either out reaches this handler as an empty string rather than being refused by
        // the boundary validator. A parameter the request did not carry is EC2's
        // `MissingParameter` — the same refusal `AssociateSecurityGroupVpc` answers for the
        // same pair of parameters.
        for (parameter, value) in [
            ("groupId", input.group_id.as_str()),
            ("vpcId", input.vpc_id.as_str()),
        ] {
            if value.is_empty() {
                return Err(state::missing_parameter(format!(
                    "The request must contain the parameter {parameter}"
                )));
            }
        }

        let service_state = state::require_service_state(&*self.engine)?;
        let mut store = service_state.lock();

        // "A VPC ID" (`$VpcId`): the VPC the group is being made unusable in is resolved
        // FIRST, in the same order the associate resolves the pair, and an id the account
        // holds no VPC for is refused with the code every operation of this crate refuses an
        // unknown VPC id with rather than reported as "not associated".
        if !store.vpcs.contains_key(input.vpc_id.as_str()) {
            return Err(state::invalid_vpc_id_not_found(&input.vpc_id));
        }

        // A value that is not a security group id names no group in ANY account, so it is
        // refused as malformed before the store is asked about it — the same order every
        // other operation of this crate reads a `GroupId` in.
        if !state::security_group_id_is_well_formed(&input.group_id) {
            return Err(state::invalid_group_id_malformed(&input.group_id));
        }
        // A group id this account holds no group for is nothing this account can
        // disassociate.
        if !store.security_groups.contains_key(input.group_id.as_str()) {
            return Err(state::invalid_group_not_found(&input.group_id));
        }

        // "Disassociates a security group from a VPC" (`DisassociateSecurityGroupVpc`): a
        // pair the account holds no association for has nothing to disassociate, which is a
        // complaint about the value of `vpcId` for this group rather than about either
        // resource — both exist. An association the clock is still taking down IS one the
        // account holds, so it is addressed below rather than refused here.
        let now = ::std::time::Instant::now();
        if !store.holds_security_group_vpc_association(&input.group_id, &input.vpc_id, now) {
            return Err(state::invalid_parameter_value(format!(
                "Value ({vpc_id}) for parameter vpcId is invalid. The security group \
                 '{group_id}' is not associated with that VPC",
                vpc_id = input.vpc_id,
                group_id = input.group_id,
            )));
        }

        // "You cannot disassociate the security group if any Elastic network interfaces in
        // the associated VPC are still associated with the security group"
        // (`DisassociateSecurityGroupVpc`): an interface in that VPC using the group still
        // depends on the association, so taking it down is refused with the code and the
        // wording EC2 answers a dependency with — naming the GROUP the request named and
        // nothing else, as every other dependency refusal of this crate does.
        if !store
            .network_interfaces_in_vpc_with_security_group(&input.vpc_id, &input.group_id)
            .is_empty()
        {
            return Err(state::dependency_violation(format!(
                "resource {group_id} has a dependent object",
                group_id = input.group_id,
            )));
        }

        // DryRun sits AFTER every id has been resolved and every refusal asked, and BEFORE
        // the write: the flag asks whether the request WOULD have succeeded, so one that
        // would have been refused is refused with that reason instead, and one that would
        // have succeeded leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // The association is left in `disassociating` with the clock owing it
        // `disassociated` one transition delay later, so a caller reading straight after
        // this call still sees `disassociating`. "The state of the disassociation"
        // (`$State`) is taken off the record that was just written, so this operation and
        // `DescribeSecurityGroupVpcAssociations` cannot report one association differently.
        let association = store
            .disassociate_security_group_vpc_association(&input.group_id, &input.vpc_id, now)
            .ok_or_else(|| {
                state::internal_error(
                    "the security group VPC association checked a moment ago could not be \
                     disassociated",
                )
            })?;
        Ok(DisassociateSecurityGroupVpcOutput {
            state: association.state,
        })
    }
}
