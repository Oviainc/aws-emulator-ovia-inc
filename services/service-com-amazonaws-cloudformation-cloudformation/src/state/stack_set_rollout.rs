// SPDX-License-Identifier: CC0-1.0

//! Rolling ONE stack-set `UPDATE` out over the stack instances it acts on.
//!
//! `UpdateStackSet` "updates the StackSet and associated stack instances in the specified
//! accounts and Amazon Web Services Regions": a selected instance's own stack is redeployed
//! from the template and the parameters now in force for it, the instance reports the update
//! as in progress until the operation concludes, and the resources of that stack are brought
//! into step in the services that own them.
//!
//! That half lives here rather than in the handler, so the operation's own file states WHAT
//! the update writes on the stack set and this module states what it does to ONE instance:
//! [`redeploy`] opens one instance's redeployment under the store lock, [`apply`] performs
//! the resource work with the store UNLOCKED, and [`record_failures`] writes what failed
//! onto the instances and the operation.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::stack_sets::{detailed_status, instance_status, operation_status};
use super::{status, ComAmazonawsCloudformationCloudformationState, StoredParameter, StoredTag};

/// What ONE instance's stack carries after the redeployment that is about to run: the
/// revision the update installs on it.
pub struct Revision<'a> {
    /// The template body the stack set now holds, which every instance deploys.
    pub template_body: &'a str,
    /// That body's PROCESSED stage, computed once off the store lock.
    pub processed_template: &'a super::template::Processed,
    /// The parameters in force on THIS instance: the stack set's, with the instance's own
    /// overrides applied.
    pub parameters: &'a [StoredParameter],
    /// The capabilities the stack set now holds.
    pub capabilities: &'a [String],
    /// The tags the stack set now holds, which the instance's resources are tagged with.
    pub tags: &'a [StoredTag],
    /// The exported values in force in the instance's own Region, which its template's
    /// `Fn::ImportValue` references resolve against.
    pub exports: &'a BTreeMap<String, String>,
}

/// WHICH instance one call of [`redeploy`] acts on, and as part of which operation: the
/// stack set that holds it (addressed the way every read of this store addresses one), the
/// account and Region pair that identifies the instance within it, the instant the update
/// started and the revision to install.
pub struct Redeploying<'a> {
    /// The Region the stack set itself belongs to, which is the Region its administrator
    /// call was signed for.
    pub stack_set_region: &'a str,
    /// The stack set holding the instance, by id.
    pub stack_set_id: &'a str,
    /// The `OperationId` of the StackSet operation this redeployment is part of.
    pub operation_id: &'a str,
    /// The target account the instance belongs to.
    pub account: &'a str,
    /// The target Region the instance's stack lives in.
    pub region: &'a str,
    /// The instant the update started, which the stack's transition is stamped with.
    pub now: SystemTime,
    /// The revision to install on the instance's stack.
    pub revision: Revision<'a>,
}

/// One instance's stack as [`redeploy`] left it, for the provisioning walk that brings its
/// resources into step with the revision now in force.
pub struct Rollout {
    /// The target account the instance belongs to.
    pub account: String,
    /// The target Region the instance's stack lives in.
    pub region: String,
    /// The stack the instance deploys, by id.
    pub stack_id: String,
    /// What the redeployed template declares, which the walk reconciles.
    pub declared: Vec<super::StoredResource>,
    /// What that stack held BEFORE this update: what the walk keeps, changes or deletes.
    pub previous: Vec<super::StoredResource>,
    /// Why this service could not deploy the template at all, for an instance whose update
    /// failed before any owning service was asked for a resource.
    pub cannot_deploy: Option<String>,
    /// Why CLOUDFORMATION refused this redeployment before anything was asked of any owning
    /// service: the template imports an export nothing publishes in the instance's Region
    /// ([`super::StoredStack::unresolved_import_failure`]).
    pub refused: Option<String>,
    /// The stack identity and tags CloudFormation gives each resource it acts on.
    pub provenance: super::provisioning::StackProvenance,
}

/// Open the redeployment `redeploying` describes: the stack of one instance of one stack
/// set, as part of one StackSet operation.
///
/// Called UNDER the store lock, once per selected instance: the instance's stack takes the
/// revision the update installs and transitions through `UPDATE_IN_PROGRESS` to
/// `UPDATE_COMPLETE` the way `UpdateStack` moves a stack it updates, and the instance's own
/// record reports the update as "currently in progress" in that account and Region until the
/// operation concludes. `None` when the instance has no stack to redeploy — one no
/// deployment produced a stack for — in which case nothing is asked of any owning service
/// for it.
pub fn redeploy(
    store: &mut ComAmazonawsCloudformationCloudformationState,
    redeploying: &Redeploying<'_>,
) -> Option<Rollout> {
    let Redeploying {
        stack_set_region,
        stack_set_id,
        operation_id,
        account,
        region,
        now,
        revision,
    } = redeploying;
    let now = *now;
    // The instance's stack lives in the instance's OWN Region, which is the Region its
    // resources were created in. The instance is read off the stack set so this function is
    // the one place that decides what a selected instance's redeployment does.
    let stack_id = store
        .resolve_stack_set(stack_set_region, stack_set_id)
        .and_then(|stack_set| stack_set.instance(account, region))
        .and_then(|instance| instance.stack_id.clone());
    let rollout = stack_id.and_then(|stack_id| {
        let stack = store.resolve_mut(region, &stack_id)?;
        let previous = stack.current_resources().to_vec();
        stack.set_template_body(
            revision.template_body.to_string(),
            revision.processed_template.clone(),
        );
        stack.parameters = revision.parameters.to_vec();
        stack.capabilities = revision.capabilities.to_vec();
        stack.tags = revision.tags.to_vec();
        stack.last_updated_time = Some(now);
        let cannot_deploy = stack.deploy_template(revision.exports).err();
        stack.begin_transition(
            now,
            status::UPDATE_IN_PROGRESS,
            status::UPDATE_COMPLETE,
            Some(operation_id.to_string()),
            Some(operation_id.to_string()),
        );
        if let Some(reason) = cannot_deploy.clone() {
            stack.fail_pending_transition(status::UPDATE_FAILED, reason);
        }
        // "Fn::ImportValue returns the value of an output exported by another stack": an
        // export name nothing publishes in the instance's own Region has no value to
        // import, so CloudFormation refuses this redeployment before anything is asked of
        // the services that own the resources.
        let refused = stack.unresolved_import_failure();
        Some(Rollout {
            account: account.to_string(),
            region: region.to_string(),
            stack_id: stack_id.clone(),
            declared: stack.current_resources().to_vec(),
            previous,
            cannot_deploy,
            refused,
            provenance: stack.provenance(),
        })
    });
    // "RUNNING: The operation in the specified account and Region is currently in progress",
    // and the operation that last touched this instance is this one — recorded whether or
    // not the instance had a stack to redeploy, since the operation acted on it either way.
    if let Some(stack_set) = store.resolve_stack_set_mut(stack_set_region, stack_set_id) {
        if let Some(instance) = stack_set
            .instances
            .iter_mut()
            .find(|held| held.is_in(account, region))
        {
            instance.detailed_status = detailed_status::RUNNING.to_string();
            instance.status_reason = None;
            instance.last_operation_id = operation_id.to_string();
        }
    }
    rollout
}

/// Bring each rollout's resources into step in the services that own them, with the store
/// UNLOCKED, and record on each stack what became of them.
///
/// Answers one entry per instance whose update FAILED — the account, the Region and the
/// reason — which is what [`record_failures`] writes onto the stack set.
pub async fn apply(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<ComAmazonawsCloudformationCloudformationState>,
    rollouts: Vec<Rollout>,
) -> Vec<(String, String, String)> {
    let mut failed: Vec<(String, String, String)> = Vec::new();
    for rollout in rollouts {
        let materialised = super::provisioning::reconcile(
            ctx,
            &rollout.region,
            &rollout.provenance,
            rollout.declared,
            &rollout.previous,
            rollout.refused,
            // A revision the rollout no longer declares a resource in is a resource
            // CloudFormation stops managing, so its own `DeletionPolicy` decides what
            // becomes of it.
            super::provisioning::Policies::Honoured,
        )
        .await;
        let mut store = state.lock();
        // A template this service could not deploy at all failed the update before any
        // service was asked for a resource, and that reason is the instance's.
        let mut reason = rollout.cannot_deploy;
        if let Some(stack) = store.resolve_mut(&rollout.region, &rollout.stack_id) {
            stack.schedule_resource_events(&materialised.transitions);
            stack.record_materialised_resources(materialised.resources);
            if let Some(failure) = materialised.failure {
                stack.fail_transition(SystemTime::now(), status::UPDATE_FAILED, failure.clone());
                reason = Some(failure);
            }
        }
        if let Some(reason) = reason {
            failed.push((rollout.account, rollout.region, reason));
        }
    }
    failed
}

/// Write what [`apply`] found onto the instances it failed on and onto the operation itself.
///
/// "OUTDATED: The stack isn't currently up to date with the StackSet because the associated
/// stack failed during a CreateStackSet or UpdateStackSet operation", and "FAILED: The
/// operation exceeded the specified failure tolerance" — which for an operation with no
/// tolerance stated is any failure at all. The resource work ran with the store unlocked, so
/// the clock may already have concluded the operation: the failure is then written onto the
/// CONCLUDED record rather than dropped, so nothing reports a success for an update that
/// failed.
pub fn record_failures(
    store: &mut ComAmazonawsCloudformationCloudformationState,
    stack_set_region: &str,
    stack_set_id: &str,
    operation_id: &str,
    failed: &[(String, String, String)],
) {
    if failed.is_empty() {
        return;
    }
    let Some(stack_set) = store.resolve_stack_set_mut(stack_set_region, stack_set_id) else {
        return;
    };
    for (account, region, reason) in failed {
        if let Some(instance) = stack_set
            .instances
            .iter_mut()
            .find(|held| held.is_in(account, region))
        {
            instance.status = instance_status::OUTDATED.to_string();
            instance.detailed_status = detailed_status::FAILED.to_string();
            instance.status_reason = Some(reason.clone());
        }
    }
    let count = i32::try_from(failed.len()).unwrap_or(i32::MAX);
    let reason = failed.first().map(|(_, _, reason)| reason.clone());
    if let Some(operation) = stack_set
        .operations
        .iter_mut()
        .find(|held| held.operation_id == operation_id)
    {
        match operation.pending.as_mut() {
            Some(pending) => {
                pending.status = operation_status::FAILED.to_string();
                pending.failed_stack_instances_count = Some(count);
                pending.status_reason = reason;
            }
            None => {
                operation.status = operation_status::FAILED.to_string();
                operation.failed_stack_instances_count = Some(count);
                operation.status_reason = reason;
            }
        }
    }
}
