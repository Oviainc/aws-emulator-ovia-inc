// SPDX-License-Identifier: CC0-1.0

//! Which stack already MANAGES a physical resource name, and the reason CloudFormation gives
//! a create that would take a name over again.
//!
//! ## The rule this module implements
//!
//! A physical resource name identifies ONE resource: CloudFormation cannot create a second
//! resource under a name a stack is already managing, so a plan that would do it is not a
//! plan that can be carried out. CloudFormation establishes that ITSELF, from its own record
//! of what the stacks of the account and Region hold, rather than from the refusal the owning
//! service would answer — so the failure is reported in CLOUDFORMATION's own words
//! ([`already_exists_in_stack`]), naming the name and the stack that holds it, with no
//! resource-handler wrapper around it.
//!
//! Two places ask it. The DEPLOYMENT asks it of the stack it is deploying, before it asks any
//! owning service to create the resource ([`taken_in_the_stack`]): two declarations of one
//! stack that resolve to one name collide with each other, and the second one fails. The
//! CHANGE SET asks it of every stack ([`managing`]): a name another stack owns is a name this
//! plan cannot adopt, which is what `ImportExistingResources` is held to.
//!
//! Nothing here reads a store: the caller holds the records (a stack's own, or the store's
//! stacks) and passes them in, so neither question needs this module to lock anything.

use super::{StoredResource, StoredStack};

/// The reason CloudFormation gives for a resource it will not create because the name is
/// already taken: the physical name, and the stack that manages the resource holding it.
///
/// The fixed text is CloudFormation's own; the variable parts are the physical resource id the
/// declaration resolved to and the `StackId` of the stack that manages it.
pub fn already_exists_in_stack(physical_resource_id: &str, stack_id: &str) -> String {
    format!("{physical_resource_id} already exists in stack {stack_id}")
}

/// The record of `recorded` whose resource holds the physical name `candidate` would be
/// created under, or `None` when nothing recorded there holds it.
///
/// A record of the SAME logical resource is never a collision: that is the resource itself,
/// which is what a deployment carries forward or replaces. A record with no physical name at
/// all cannot collide with anything, and the question is asked per TYPE, because a physical
/// name identifies a resource of one type.
pub fn taken_in_the_stack<'a>(
    candidate: &StoredResource,
    recorded: impl IntoIterator<Item = &'a StoredResource>,
) -> Option<&'a StoredResource> {
    if candidate.physical_resource_id.is_empty() {
        return None;
    }
    recorded.into_iter().find(|existing| {
        existing.materialised
            && existing.logical_id != candidate.logical_id
            && existing.resource_type == candidate.resource_type
            && existing.physical_resource_id == candidate.physical_resource_id
    })
}

/// The live stack of `region` that manages a resource of `resource_type` under
/// `physical_resource_id`, or `None` when no stack does.
///
/// A stack whose deletion has completed manages nothing, so it is not an owner; everything
/// else a stack records as a resource it deployed is managed by it.
pub fn managing<'a>(
    stacks: impl IntoIterator<Item = &'a StoredStack>,
    region: &str,
    resource_type: &str,
    physical_resource_id: &str,
) -> Option<&'a StoredStack> {
    if physical_resource_id.is_empty() {
        return None;
    }
    stacks.into_iter().find(|stack| {
        stack.is_live()
            && stack.region == region
            && stack.resources.iter().any(|deployed| {
                deployed.resource.materialised
                    && deployed.resource.resource_type == resource_type
                    && deployed.resource.physical_resource_id == physical_resource_id
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the reason names the physical name and the stack that manages it, in the words
    /// CloudFormation words it in.
    #[test]
    fn the_reason_names_the_name_and_the_owning_stack() {
        let rows: &[(&str, &str, &str, &str)] = &[
            (
                "a parameter name",
                "/app/version",
                "arn:aws:cloudformation:us-east-1:000000000000:stack/owner/0",
                "/app/version already exists in stack \
                 arn:aws:cloudformation:us-east-1:000000000000:stack/owner/0",
            ),
            (
                "a bucket name",
                "reports",
                "arn:aws:cloudformation:eu-west-1:000000000000:stack/other/1",
                "reports already exists in stack \
                 arn:aws:cloudformation:eu-west-1:000000000000:stack/other/1",
            ),
        ];
        for (case, physical_resource_id, stack_id, expected) in rows {
            let reason = already_exists_in_stack(physical_resource_id, stack_id);
            assert_eq!(
                reason, *expected,
                "case {case:?}: expected {expected:?}, got {reason:?}"
            );
        }
    }

    /// RULE: a name is taken by ANOTHER resource of the same type that really exists — never
    /// by the resource itself, by a record nothing created, by a resource of another type or
    /// by an empty name.
    #[test]
    fn a_name_is_taken_only_by_another_existing_resource_of_the_type() {
        let resource = |logical_id: &str, resource_type: &str, physical: &str, made: bool| {
            StoredResource {
                logical_id: logical_id.to_string(),
                resource_type: resource_type.to_string(),
                physical_resource_id: physical.to_string(),
                attributes: ::std::collections::BTreeMap::new(),
                metadata: None,
                properties: ::serde_json::Value::Null,
                materialised: made,
                deletion_policy: None,
            }
        };
        let parameter = "AWS::SSM::Parameter";
        let rows: &[(&str, StoredResource, Vec<StoredResource>, Option<&str>)] = &[
            (
                "another declaration resolving to the same name",
                resource("B", parameter, "/one", false),
                vec![resource("A", parameter, "/one", true)],
                Some("A"),
            ),
            (
                "the resource's own record",
                resource("A", parameter, "/one", false),
                vec![resource("A", parameter, "/one", true)],
                None,
            ),
            (
                "a record of a resource nothing created",
                resource("B", parameter, "/one", false),
                vec![resource("A", parameter, "/one", false)],
                None,
            ),
            (
                "the same name under another type",
                resource("B", "AWS::S3::Bucket", "/one", false),
                vec![resource("A", parameter, "/one", true)],
                None,
            ),
            (
                "a resource with no name of its own",
                resource("B", parameter, "", false),
                vec![resource("A", parameter, "", true)],
                None,
            ),
        ];
        for (case, candidate, recorded, expected) in rows {
            let taken = taken_in_the_stack(candidate, recorded.iter());
            assert_eq!(
                taken.map(|existing| existing.logical_id.as_str()),
                *expected,
                "case {case:?}: expected {expected:?}, got {taken:?}"
            );
        }
    }
}
