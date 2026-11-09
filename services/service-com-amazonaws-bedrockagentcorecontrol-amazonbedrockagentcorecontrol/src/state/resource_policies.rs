// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy a resource of this service carries: the document
//! `PutResourcePolicy` attaches to one resource, which `GetResourcePolicy`
//! reports and `DeleteResourcePolicy` removes.
//!
//! A policy belongs to the resource its ARN names, so it is keyed by that
//! resource rather than by the ARN string: `<arn resource type>/<id>`, which is
//! exactly the resource part of the ARN the request addressed. A request reaches
//! the key through [`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::resource_policy_target`],
//! which resolves an ARN only to a resource this service actually holds, so a
//! stored policy is never reported for a resource that was never created.
//!
//! The feature covers two of this service's resources — "This feature is
//! currently available only for AgentCore Runtime and Gateway"
//! (`smithy_query(query="doc",shape="PutResourcePolicy")`) — so those are the
//! only two ARN resource types that resolve, and the policy map is bounded by
//! the two stores it keys into ([`MAX_AGENT_RUNTIMES`] + [`MAX_GATEWAYS`]): one
//! entry per resource at most, and the entry of a resource that is gone is
//! dropped by [`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::drop_orphaned_resource_policies`].

use super::*;

/// The ARN resource type of an AgentCore Runtime, as [`runtime_arn`] renders it.
pub const POLICY_TARGET_RUNTIME: &str = "runtime";

/// The ARN resource type of a Gateway, as [`gateway_arn`] renders it.
pub const POLICY_TARGET_GATEWAY: &str = "gateway";

/// The modelled not-found error for a resource policy this service does not
/// hold, in the words AWS answers a policy lookup that resolves no document
/// with ("Resource Policy doesn't exist for requested arn").
///
/// The sentence has no variable part: it names the ARN the request asked under
/// rather than quoting the caller's own value, so one answer covers every
/// resource of this service that carries no policy.
///
/// It is NOT the answer for an ARN that names no resource at all — that one is
/// [`resource_arn_not_found`]: the two conditions have different remedies (write
/// a policy, versus address a resource that exists), so they are worded apart.
pub fn resource_policy_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Resource Policy doesn't exist for requested arn".to_string()),
    }
}

/// The modelled not-found error for a `resourceArn` that names no resource a
/// policy of this service can attach to, in the words AWS answers it with
/// ("ResourceArn provided in the request does not exist").
///
/// Entirely fixed text: the ARN is the caller's own value and the remedy — name
/// a resource that exists — does not depend on it. Shared by every
/// resource-policy operation, so the same unresolvable ARN is reported in one
/// sentence whichever of them was asked.
pub fn resource_arn_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("ResourceArn provided in the request does not exist".to_string()),
    }
}

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// The resource `resource_arn` names, as the key a policy of it is held
    /// under — `None` when this service holds no such resource, or when the
    /// feature does not cover its type.
    ///
    /// The ARN is resolved rather than parsed loosely: the resource part names
    /// the type and the id, and the whole ARN must be the one this service
    /// publishes for that resource in the region the request was signed for
    /// ([`runtime_arn`] / [`gateway_arn`]), so a partition, account or region
    /// that is not this emulator's names no resource here.
    pub fn resource_policy_target(&self, region: &str, resource_arn: &str) -> Option<String> {
        let (_, resource) = resource_arn.rsplit_once(':')?;
        let (resource_type, resource_id) = resource.split_once('/')?;
        let published = match resource_type {
            POLICY_TARGET_RUNTIME if self.agent_runtimes.contains_key(resource_id) => {
                runtime_arn(region, resource_id)
            }
            POLICY_TARGET_GATEWAY if self.gateways.contains_key(resource_id) => {
                gateway_arn(region, resource_id)
            }
            _ => return None,
        };
        (published == resource_arn).then(|| resource.to_string())
    }

    /// Attach `policy` to the resource `target` names, replacing whatever
    /// document that resource carried: one resource has one resource policy, so
    /// a second write is the new document rather than a second policy.
    pub fn attach_resource_policy(&mut self, target: String, policy: String) {
        self.resource_policies.insert(target, policy);
    }

    /// The document the resource `target` names carries, if it carries one.
    pub fn resource_policy(&self, target: &str) -> Option<&str> {
        self.resource_policies.get(target).map(String::as_str)
    }

    /// Remove the document the resource `target` names carries, answering it
    /// when there was one to remove.
    pub fn detach_resource_policy(&mut self, target: &str) -> Option<String> {
        self.resource_policies.remove(target)
    }

    /// Drop the policy of every resource this service no longer holds.
    ///
    /// A resource-based policy exists only as a property of its resource, so a
    /// resource that has been deleted — including one whose removal the clock
    /// made due — carries none: the removal was owed at the same instant as the
    /// resource's own, whether or not anybody called. Nothing a later caller can
    /// reach changes, because a policy is only ever read through
    /// [`Self::resource_policy_target`], which resolves no deleted resource.
    pub fn drop_orphaned_resource_policies(&mut self) {
        let orphaned: Vec<String> = self
            .resource_policies
            .keys()
            .filter(|target| !self.holds_resource_policy_target(target))
            .cloned()
            .collect();
        for target in orphaned {
            self.resource_policies.remove(&target);
        }
    }

    /// Whether this service still holds the resource `target` names.
    fn holds_resource_policy_target(&self, target: &str) -> bool {
        match target.split_once('/') {
            Some((POLICY_TARGET_RUNTIME, resource_id)) => {
                self.agent_runtimes.contains_key(resource_id)
            }
            Some((POLICY_TARGET_GATEWAY, resource_id)) => self.gateways.contains_key(resource_id),
            _ => false,
        }
    }
}
