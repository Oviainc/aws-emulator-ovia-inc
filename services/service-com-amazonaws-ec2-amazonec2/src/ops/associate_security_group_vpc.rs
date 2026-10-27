// SPDX-License-Identifier: CC0-1.0

//! @op AssociateSecurityGroupVpc
//! Associates one of the account's security groups with a VPC it was not created in, so the
//! same group can be used by network interfaces and instances in that VPC.
//!
//! The association is the (`GroupId`, `VpcId`) PAIR and nothing else, so it is stored as
//! that pair in [`crate::state::ComAmazonawsEc2Amazonec2State::security_group_vpc_associations`]
//! and read back by `DescribeSecurityGroupVpcAssociations` through the one view both
//! operations report it with. It is answered in the non-terminal `associating` state, with
//! the clock — not a second call — owing it `associated`.
//!
//! Implemented here together with its reader `DescribeSecurityGroupVpcAssociations`, whose
//! own stub issue this change closes: without the write there is nothing for that read to
//! report. `DisassociateSecurityGroupVpc` stays a stub, with its own issue.
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
//! # Modeled contract for `AssociateSecurityGroupVpc`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Associates a security group with another VPC in the same Region. This enables you to use
//! the same security group with network interfaces and instances in the specified VPC. The
//! VPC you want to associate the security group with must be in the same Region. You can
//! associate the security group with another VPC if your account owns the VPC or if the VPC
//! was shared with you. You must own the security group. You cannot use this feature with
//! default security groups. You cannot use this feature with the default VPC.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `GroupId` (SecurityGroupId) **required** — A security group ID.
//! * `VpcId` (VpcId) **required** — A VPC ID.
//!
//! ## Output
//!
//! * `State` (SecurityGroupVpcAssociationState, one of: associated | associating |
//!   association-failed | disassociated | disassociating | disassociation-failed) — The state
//!   of the association.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateSecurityGroupVpcError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateSecurityGroupVpcError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#AssociateSecurityGroupVpc")]
#[derive(Debug)]
pub enum AssociateSecurityGroupVpcErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_security_group_vpc::AssociateSecurityGroupVpcError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateSecurityGroupVpcError = crate::scaffolded::Scaffolded<AssociateSecurityGroupVpcErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateSecurityGroupVpcErrorModeled> for AssociateSecurityGroupVpcError {
    fn from(modeled: AssociateSecurityGroupVpcErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateSecurityGroupVpc` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateSecurityGroupVpc<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateSecurityGroupVpc<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateSecurityGroupVpc<Eng>
{
    type Input = AssociateSecurityGroupVpcInput;
    type Output = AssociateSecurityGroupVpcOutput;
    type Error = AssociateSecurityGroupVpcError;

    const NAME: &'static str = "AssociateSecurityGroupVpc";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_SECURITY_GROUP_VPC_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_SECURITY_GROUP_VPC_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; every fact this association reports comes
        // from the request and from the store the engine on `self` holds.
        let _ = ctx;

        // Both ids are `@required` but modelled `clientOptional`, so a request that left
        // either out reaches this handler as an empty string rather than being refused by
        // the boundary validator. A parameter the request did not carry is EC2's
        // `MissingParameter`.
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

        // "A VPC ID" (`$VpcId`): the VPC the group is being made usable in is what this
        // operation is asked about, so it is resolved FIRST, and an id the account holds no
        // VPC for is refused with the code every operation of this crate refuses an unknown
        // VPC id with rather than associating with a VPC that does not exist.
        let Some(vpc) = store.vpcs.get(input.vpc_id.as_str()) else {
            return Err(state::invalid_vpc_id_not_found(&input.vpc_id));
        };

        // "You cannot use this feature with the default VPC" (`AssociateSecurityGroupVpc`):
        // the VPC exists and the request is well formed, but what the VPC IS forbids the
        // action, which is EC2's `OperationNotPermitted`.
        if vpc.is_default == Some(true) {
            return Err(crate::scaffolded::Scaffolded::operation_not_permitted(
                format!(
                    "The vpc '{vpc_id}' is the default VPC, which a security group cannot be \
                     associated with",
                    vpc_id = input.vpc_id,
                ),
            ));
        }

        // A value that is not a security group id names no group in ANY account, so it is
        // refused as malformed before the store is asked about it — the same order every
        // other operation of this crate reads a `GroupId` in.
        if !state::security_group_id_is_well_formed(&input.group_id) {
            return Err(state::invalid_group_id_malformed(&input.group_id));
        }
        // "You must own the security group" (`AssociateSecurityGroupVpc`): a group id this
        // account holds no group for is nothing this account can associate.
        let Some(group) = store.security_groups.get(input.group_id.as_str()) else {
            return Err(state::invalid_group_not_found(&input.group_id));
        };
        let group_vpc_id = group.vpc_id.clone().unwrap_or_default();

        // "You cannot use this feature with default security groups"
        // (`AssociateSecurityGroupVpc`) — refused on the same terms as the default VPC
        // above: an attribute of the resource, not of the request, forbids the action.
        if store.is_default_security_group(&input.group_id) {
            return Err(crate::scaffolded::Scaffolded::operation_not_permitted(
                format!(
                    "The security group '{group_id}' is a default security group, which \
                     cannot be associated with another VPC",
                    group_id = input.group_id,
                ),
            ));
        }

        // "Associates a security group with ANOTHER VPC in the same Region"
        // (`AssociateSecurityGroupVpc`): a group is already usable in the VPC it was created
        // in, so that VPC is not a value this parameter takes.
        if group_vpc_id == input.vpc_id {
            return Err(state::invalid_parameter_value(format!(
                "Value ({vpc_id}) for parameter vpcId is invalid. The security group \
                 '{group_id}' was created in that VPC",
                vpc_id = input.vpc_id,
                group_id = input.group_id,
            )));
        }

        // An association IS the pair, so a pair the account already holds one for has
        // nothing to establish: a repeat is refused with `Resource.AlreadyAssociated`
        // rather than restarting the association's clock, which would move an `associated`
        // association back to `associating` for every later reader.
        if store
            .security_group_vpc_associations
            .contains_key(&(input.group_id.clone(), input.vpc_id.clone()))
        {
            return Err(state::resource_already_associated(format!(
                "The security group '{group_id}' is already associated with the vpc \
                 '{vpc_id}'",
                group_id = input.group_id,
                vpc_id = input.vpc_id,
            )));
        }

        if !store.has_security_group_vpc_association_capacity() {
            return Err(state::resource_limit_exceeded(format!(
                "This emulator holds {max} security group VPC associations, which is as many \
                 as it stores",
                max = state::MAX_SECURITY_GROUP_VPC_ASSOCIATIONS,
            )));
        }

        // DryRun sits AFTER every id has been resolved and every refusal asked, and BEFORE
        // the write: the flag asks whether the request WOULD have succeeded, so one that
        // would have been refused is refused with that reason instead, and one that would
        // have succeeded leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // The association is written in `associating` with the clock owing it `associated`
        // one transition delay later, so a caller reading straight after this call still
        // sees `associating`. "The state of the association" (`$State`) is taken off the
        // record that was just stored, so this operation and
        // `DescribeSecurityGroupVpcAssociations` cannot report one association differently.
        let now = ::std::time::Instant::now();
        let association =
            store.insert_security_group_vpc_association(&input.group_id, &input.vpc_id, now);
        Ok(AssociateSecurityGroupVpcOutput {
            state: association.state,
        })
    }
}
