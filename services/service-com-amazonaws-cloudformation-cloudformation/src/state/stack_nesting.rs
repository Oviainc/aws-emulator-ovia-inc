// SPDX-License-Identifier: CC0-1.0

//! Which stack a stack is NESTED IN: the parent/root relation every read of a stack reports,
//! derived from the records the store already holds.
//!
//! A stack created by an `AWS::CloudFormation::Stack` resource is a stack of its own
//! ([`super::provisioning`]'s nested-stack provider creates it through CloudFormation's own
//! `CreateStack`), and the resource that created it holds its stack id as the resource's
//! physical id — which is exactly what `DescribeStackResource` reports for the type. So the
//! relation is already recorded, once, where CloudFormation records it: the PARENT of a stack
//! is the stack holding a resource of that type whose physical id is this stack's id, and the
//! ROOT is the top of that chain ("the stack ID of the top-level stack to which the nested
//! stack ultimately belongs").
//!
//! Derived here rather than copied onto each stack so the two can never disagree: a resource
//! record and a parent pointer would be two statements of one fact, and the resource record
//! is the one the per-resource reads already answer from.
//!
//! The derivation is read as ONE index per read ([`Nesting`]): a projection reports the
//! relation for every stack of a page, so the records are walked once for the page rather than
//! once per stack.

use std::collections::BTreeMap;

use super::{ComAmazonawsCloudformationCloudformationState, StoredStack, STACK_RESOURCE_TYPE};

/// How deep a chain of parents is walked before the walk gives up.
///
/// A stack's parent chain is finite by construction — each nested stack is created by a
/// resource of the stack above it — and this is the bound that keeps a read finite even for
/// records that somehow point at each other. It is past the deepest nesting a template can
/// reach: each level's generated stack name carries its parent's whole name plus a separator,
/// a logical id and a unique segment, so a chain this long cannot be built under the
/// documented stack-name maximum ([`super::STACK_NAME_MAX_LEN`]).
const MAX_PARENT_WALK: usize = 16;

/// The nesting relation of one Region, read once: for each stack that an
/// `AWS::CloudFormation::Stack` resource created, the stack id of the stack that created it.
#[derive(Debug, Default)]
pub struct Nesting {
    /// Keyed by the CHILD's stack id, which is the resource's physical id.
    parent_of: BTreeMap<String, String>,
}

impl Nesting {
    /// `StackSummary$ParentId` / `Stack$ParentId` for one stack: "the stack ID of the direct
    /// parent of this stack", or `None` for a stack no `AWS::CloudFormation::Stack` resource
    /// created — a top-level stack, which carries neither member.
    pub fn parent_id_of(&self, stack: &StoredStack) -> Option<String> {
        self.parent_of.get(&stack.stack_id).cloned()
    }

    /// `StackSummary$RootId` / `Stack$RootId`: "the stack ID of the top-level stack to which
    /// the nested stack ultimately belongs" — the last stack the parent chain reaches.
    ///
    /// `None` for a stack with no parent: a top-level stack is the root of nothing it reports,
    /// so it carries neither member. For a first-level child the root IS the parent ("for the
    /// first level of nested stacks, the root stack is also the parent stack").
    pub fn root_id_of(&self, stack: &StoredStack) -> Option<String> {
        let mut root = self.parent_id_of(stack)?;
        for _ in 0..MAX_PARENT_WALK {
            match self.parent_of.get(&root) {
                Some(above) => root = above.clone(),
                None => break,
            }
        }
        Some(root)
    }
}

impl ComAmazonawsCloudformationCloudformationState {
    /// The nesting relation of one Region ([`Nesting`]), built in ONE pass over that Region's
    /// stacks and their resource records.
    ///
    /// The relation is looked for within the Region: a nested stack is created by the parent's
    /// deployment, in the Region that deployment ran in.
    pub fn nesting_in(&self, region: &str) -> Nesting {
        let mut parent_of = BTreeMap::new();
        for parent in self.stacks.iter().filter(|stack| stack.region == region) {
            for deployed in &parent.resources {
                if deployed.resource.resource_type != STACK_RESOURCE_TYPE {
                    continue;
                }
                let child = &deployed.resource.physical_resource_id;
                if child.is_empty() || child == &parent.stack_id {
                    continue;
                }
                parent_of.insert(child.clone(), parent.stack_id.clone());
            }
        }
        Nesting { parent_of }
    }
}
