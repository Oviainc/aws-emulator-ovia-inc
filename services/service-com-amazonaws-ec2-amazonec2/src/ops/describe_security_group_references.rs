// SPDX-License-Identifier: CC0-1.0

//! @op DescribeSecurityGroupReferences
//! Reports the VPCs whose security groups hold a rule referencing the security groups the
//! request names — read out of the rules `AuthorizeSecurityGroupIngress` and
//! `AuthorizeSecurityGroupEgress` store, so this operation and `DescribeSecurityGroups`
//! cannot disagree about which group references which.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it, and this one only READS it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeSecurityGroupReferences`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Describes the VPCs on the other side of a VPC peering or Transit Gateway connection that
//! are referencing the security groups you've specified in this request.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `GroupId` (GroupIds) **required** — The IDs of the security groups in your account.
//!
//! ## Output
//!
//! * `SecurityGroupReferenceSet` (SecurityGroupReferences) — Information about the VPCs with
//!   the referencing security groups.

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::state::MAX_RESOURCE_IDS;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeSecurityGroupReferencesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeSecurityGroupReferencesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DescribeSecurityGroupReferences")]
#[derive(Debug)]
pub enum DescribeSecurityGroupReferencesErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_security_group_references::DescribeSecurityGroupReferencesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeSecurityGroupReferencesError = crate::scaffolded::Scaffolded<DescribeSecurityGroupReferencesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeSecurityGroupReferencesErrorModeled> for DescribeSecurityGroupReferencesError {
    fn from(modeled: DescribeSecurityGroupReferencesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeSecurityGroupReferences` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeSecurityGroupReferences<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeSecurityGroupReferences<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeSecurityGroupReferences<Eng>
{
    type Input = DescribeSecurityGroupReferencesInput;
    type Output = DescribeSecurityGroupReferencesOutput;
    type Error = DescribeSecurityGroupReferencesError;

    const NAME: &'static str = "DescribeSecurityGroupReferences";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_SECURITY_GROUP_REFERENCES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_SECURITY_GROUP_REFERENCES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries request metadata only; this operation reads none of it. The store
        // is reached through the engine held on `self`.
        let _ = ctx;

        // "Checks whether you have the required permissions for the action, without
        // actually making the request, and provides an error response. If you have the
        // required permissions, the error response is DryRunOperation"
        // (`DescribeSecurityGroupReferencesRequest$DryRun`). Every caller of this emulator
        // is permitted, so a dry run reports that outcome and reads nothing.
        if input.dry_run.unwrap_or(false) {
            return Err(crate::state::dry_run_operation());
        }

        let state = crate::state::require_service_state(&*self.engine)?;

        // "The IDs of the security groups in your account"
        // (`DescribeSecurityGroupReferencesRequest$GroupId`, required): the report is ABOUT
        // the groups the request names, so a request that names none asks nothing, and is
        // told which parameter it left out rather than answered with an empty set. A request
        // that leaves the member out ALTOGETHER is already refused by the boundary
        // validator; an empty list is not an absent member, so it is refused here.
        let requested: &[String] = &input.group_id;
        if requested.is_empty() {
            return Err(crate::state::missing_parameter(
                "The request must contain the parameter groupId",
            ));
        }
        // The list is caller-sized and every entry is resolved against the store, so it is
        // capped the way every other EC2 operation of this crate caps a resource-id list,
        // before any work proportional to it is done and before the store is locked.
        if requested.len() > MAX_RESOURCE_IDS {
            return Err(crate::state::invalid_parameter_value(format!(
                "GroupId carries {} ids, above the maximum of {MAX_RESOURCE_IDS}",
                requested.len()
            )));
        }

        let store = state.lock();

        // Each named group is resolved against the store: a value that is not a security
        // group id names no group in any account, and an id no group of the account answers
        // to is an error rather than an absent row — the same two refusals every operation
        // of this crate that resolves a `GroupId` answers, in the same order.
        //
        // The VPC each named group sits in is kept here: whether a reference crosses a VPC
        // boundary is what makes it one of this operation's references, so it is read once
        // per named group rather than per candidate rule.
        let mut named_vpcs: BTreeMap<&str, Option<&str>> = BTreeMap::new();
        for group_id in requested {
            if !crate::state::security_group_id_is_well_formed(group_id) {
                return Err(crate::state::invalid_group_id_malformed(group_id));
            }
            let group = store
                .security_groups
                .get(group_id.as_str())
                .ok_or_else(|| crate::state::invalid_group_not_found(group_id))?;
            named_vpcs.insert(group_id.as_str(), group.vpc_id.as_deref());
        }

        // "Describes the VPCs on the other side of a VPC peering or Transit Gateway
        // connection that are referencing the security groups you've specified in this
        // request" (`DescribeSecurityGroupReferences`): the store is walked ONCE, and every
        // rule of every group of the account that references a named group FROM ANOTHER VPC
        // contributes the VPC that holds it. A reference inside the named group's own VPC
        // crosses no connection, so it is no VPC "on the other side" of one and is not
        // reported — which is what keeps a group's own self-reference, and the one AWS gives
        // a VPC's default group, out of the answer.
        //
        // Walking the groups rather than the request keeps the cost of the whole report
        // linear in the stored rules plus the ids the request named, instead of the product
        // of the two.
        let mut by_group: BTreeMap<&str, BTreeSet<ReferencingVpc>> = BTreeMap::new();
        for holder in store.security_groups.values() {
            // A group with no VPC is in no VPC that could be reported as the referencing
            // one, so its rules reference nothing across a connection.
            let Some(holder_vpc) = holder.vpc_id.as_deref() else {
                continue;
            };
            let permissions = holder
                .ip_permissions
                .iter()
                .flatten()
                .chain(holder.ip_permissions_egress.iter().flatten());
            for permission in permissions {
                for pair in permission.user_id_group_pairs.iter().flatten() {
                    let Some(referenced) = referenced_group_id(&store, holder_vpc, pair) else {
                        continue;
                    };
                    // Only the groups the request named are reported on, and only from
                    // outside the VPC that holds them.
                    let Some((group_id, referenced_vpc)) =
                        named_vpcs.get_key_value(referenced.as_str())
                    else {
                        continue;
                    };
                    if *referenced_vpc == Some(holder_vpc) {
                        continue;
                    }
                    by_group
                        .entry(*group_id)
                        .or_default()
                        .insert(ReferencingVpc {
                            vpc_id: holder_vpc.to_string(),
                            // "The ID of the VPC peering connection (if applicable)"
                            // (`SecurityGroupReference$VpcPeeringConnectionId`): the
                            // connection the referencing RULE records
                            // (`UserIdGroupPair$VpcPeeringConnectionId`), and no connection
                            // at all for a rule that records none.
                            vpc_peering_connection_id: pair
                                .vpc_peering_connection_id
                                .clone()
                                .filter(|id| !id.is_empty()),
                        });
                }
            }
        }

        // One entry per referencing VPC and connection, in the order the request named the
        // groups — a group named twice is answered once, since the request asks about a
        // GROUP rather than about a position in a list.
        let mut reported: BTreeSet<&str> = BTreeSet::new();
        let mut references: Vec<SecurityGroupReference> = Vec::new();
        for group_id in requested {
            if !reported.insert(group_id.as_str()) {
                continue;
            }
            let Some(referencing) = by_group.get(group_id.as_str()) else {
                continue;
            };
            references.extend(referencing.iter().map(|vpc| SecurityGroupReference {
                // "The ID of your security group"
                // (`SecurityGroupReference$GroupId`): the group that is REFERENCED, which
                // is the one the request named.
                group_id: Some(group_id.clone()),
                referencing_vpc_id: Some(vpc.vpc_id.clone()),
                vpc_peering_connection_id: vpc.vpc_peering_connection_id.clone(),
                // "The ID of the transit gateway (if applicable)"
                // (`SecurityGroupReference$TransitGatewayId`): a reference carried by a
                // transit gateway is one made across a VPC attachment to it, and no
                // operation of this crate stores such an attachment yet, so nothing the
                // store holds names a transit gateway here. When this crate starts storing
                // VPC attachments, that store is the one thing to consult for this member.
                transit_gateway_id: None,
            }));
        }

        Ok(DescribeSecurityGroupReferencesOutput {
            // "Information about the VPCs with the referencing security groups"
            // (`DescribeSecurityGroupReferencesResult$SecurityGroupReferenceSet`): reported
            // empty rather than left out when nothing references the named groups, so a
            // caller reads the member the same way either way.
            security_group_reference_set: Some(references),
        })
    }
}

/// One VPC that references a named security group, and the connection the referencing rule
/// records it reaches the group over.
///
/// The pair the report is DEDUPLICATED on: "describes a VPC with a security group that
/// references your security group" (`SecurityGroupReference`) is one record per referencing
/// VPC, however many of that VPC's rules name the group. Ordered, so the entries of one
/// named group are reported in a stable order across reads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReferencingVpc {
    /// "The ID of the VPC with the referencing security group"
    /// (`SecurityGroupReference$ReferencingVpcId`): the VPC of the group that HOLDS the
    /// rule.
    vpc_id: String,
    /// The VPC peering connection the rule records, if it records one.
    vpc_peering_connection_id: Option<String>,
}

/// The `GroupId` of the security group the reference `pair` names, held by a rule of a group
/// in `holder_vpc` — or `None` when it names no group this store can resolve.
///
/// Resolved the way the rule names it — "[Default VPC] The name of the security group. For a
/// security group in a nondefault VPC, use the security group ID"
/// (`UserIdGroupPair$GroupName`) — so a reference written by name answers for the same group
/// as one written by id, and never out of the pair's own `VpcId`, which is "the ID of the VPC
/// for the referenced security group" (`UserIdGroupPair$VpcId`) and need not be stored on the
/// rule at all.
fn referenced_group_id(
    store: &crate::state::ComAmazonawsEc2Amazonec2State,
    holder_vpc: &str,
    pair: &UserIdGroupPair,
) -> Option<String> {
    match pair.group_id.as_deref().filter(|id| !id.is_empty()) {
        Some(group_id) => Some(group_id.to_string()),
        None => pair
            .group_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .and_then(|name| store.security_group_id_for_name(holder_vpc, name)),
    }
}
