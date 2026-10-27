// SPDX-License-Identifier: CC0-1.0

//! The stack policy in force on a stack: which of an update's planned resource actions it
//! permits, and how a denied one is reported.
//!
//! A stack policy is "a JSON document that defines the update actions that can be performed
//! on designated resources". `SetStackPolicy` (and `UpdateStack`'s own `StackPolicyBody`)
//! associates one with a live stack, and every later update is held to it: CloudFormation
//! decides, per resource the update would ACT ON, whether the policy permits the action it
//! plans there.
//!
//! The actions a statement names are the update actions a stack policy governs
//! ([`update_action`]), and the resource it names is a logical resource of the stack
//! ([`RESOURCE_PREFIX`]) — so the plan this module is evaluated against is the resource-level
//! change list [`super::change_plan`] already computes for a change set: a `Modify` that
//! recreates the resource is an `Update:Replace`, one that does not is an `Update:Modify`,
//! and a `Remove` is an `Update:Delete`. A resource the update ADDS is not one the policy
//! governs: it does not exist yet, so no update action is performed on it.
//!
//! The decision is the policy language's, and it is the tree's ONE evaluation of that
//! language that makes it: `services/common`'s [`::service_common::iam_authorization`],
//! asked as a RESOURCE-attached document, which is the shape a stack policy's statements
//! have (a `Principal`, which the grammar requires to be the wildcard, and a `Resource`
//! naming what the statement protects). So an explicit `Deny` that matches wins over every
//! `Allow` wherever it stands, the `Not…` spellings, the `*`/`?` wildcards and the string
//! condition operators are that module's, and the statement a denial names is the one it
//! attributes the decision to. What is this service's own is only the reading of a decision
//! that STATES nothing: setting a stack policy protects the stack's resources, so an action
//! no statement allows is denied too — which is why every documented stack policy that means
//! to permit anything carries an explicit `Allow` statement for it.
//!
//! Nothing here reads or writes the store: the handler that is deploying computes the plan,
//! asks this module for the verdict, and fails the update itself. Both of the operations that
//! deploy an update to a LIVE stack — `UpdateStack` and `ExecuteChangeSet` — ask, each with
//! the plan its own request would carry out, so one policy decides them alike.

use ::service_common::iam_authorization::{
    resource_outcome_prepared, Decision, PreparedDocument, Request,
};

use super::change_action;
use super::change_plan::{replacement, StoredResourceChange};
use super::provisioning::ResourceTransition;
use super::resource_status;

/// The update actions a stack policy statement names, as the policy grammar spells them.
pub mod update_action {
    /// "Update:Modify — Permission to update resources that will keep the same physical
    /// resource id."
    pub const MODIFY: &str = "Update:Modify";
    /// "Update:Replace — Permission to update resources that will be replaced with a new
    /// physical resource id."
    pub const REPLACE: &str = "Update:Replace";
    /// "Update:Delete — Permission to update resources that will be deleted."
    pub const DELETE: &str = "Update:Delete";
}

/// How a stack policy statement names the resource it protects: the logical resource id of
/// a resource of that stack, as the `Resource` member spells it.
pub const RESOURCE_PREFIX: &str = "LogicalResourceId/";

/// The policy document member whose absence makes a body one this service cannot evaluate
/// as a stack policy at all.
const STATEMENT: &str = "Statement";

/// The one condition KEY a stack policy's statements are documented to be conditioned on:
/// "you can use the `ResourceType` condition key to apply a statement to resources of a
/// given type".
const RESOURCE_TYPE_KEY: &str = "ResourceType";

/// The condition keys a planned resource action MODELS — the one above, whose value every
/// such action carries — so a statement conditioned on it is decided rather than left
/// undecided by the evaluator.
const PUBLISHED_CONDITION_KEYS: &[&str] = &[RESOURCE_TYPE_KEY];

/// One update action an update plans to perform on one of the stack's resources — what the
/// stack policy is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAction {
    /// The resource's logical id, which is what a statement's `Resource` names.
    pub logical_id: String,
    /// The resource's type, which is what the `ResourceType` condition compares.
    pub resource_type: String,
    /// The identifier the resource carries now, reported on the event of an action that was
    /// denied: the resource keeps it, since nothing was done to it.
    pub physical_resource_id: Option<String>,
    /// The action itself ([`update_action`]).
    pub action: &'static str,
}

/// The update actions a resource-level change list plans, in the order the plan reports
/// them.
///
/// The mapping is the one the action names state: a change that recreates the resource
/// ("will be replaced with a new physical resource id") is an `Update:Replace`, one that
/// keeps it is an `Update:Modify`, and a resource the update removes is an `Update:Delete`.
/// A `Replacement` of `Conditional` is a change that MAY recreate the resource, so it is
/// held to the same permission as one that always does.
pub fn planned_actions(changes: &[StoredResourceChange]) -> Vec<PlannedAction> {
    changes
        .iter()
        .filter_map(|change| {
            let action = match change.action.as_str() {
                change_action::MODIFY => match change.replacement.as_deref() {
                    Some(replacement::TRUE | replacement::CONDITIONAL) => update_action::REPLACE,
                    _ => update_action::MODIFY,
                },
                change_action::REMOVE => update_action::DELETE,
                // An `Add`: the resource does not exist, so the update performs no update
                // action on it.
                _ => return None,
            };
            Some(PlannedAction {
                logical_id: change.logical_resource_id.clone(),
                resource_type: change.resource_type.clone(),
                physical_resource_id: change.physical_resource_id.clone(),
                action,
            })
        })
        .collect()
}

/// An update action the stack policy in force does not permit: which resource, which
/// action, and the statement that denied it where one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The index of the `Statement` that denies the action, or `None` where no statement
    /// allows it.
    pub statement: Option<usize>,
    /// The action the policy was asked about.
    pub action: &'static str,
    /// The resource the action would have been performed on.
    pub logical_id: String,
    /// That resource's type.
    pub resource_type: String,
    /// The identifier that resource keeps, the action not having been performed.
    pub physical_resource_id: Option<String>,
}

impl Denial {
    /// What the denied resource's own `UPDATE_FAILED` event reports: the statement that
    /// denied the action, the action, and the resource it was to be performed on.
    pub fn reason(&self) -> String {
        match self.statement {
            Some(index) => format!(
                "Action denied by stack policy: Statement [#{index}] does not allow \
                 [{}] for resource [{RESOURCE_PREFIX}{}];",
                self.action, self.logical_id
            ),
            // No statement denies the action and none allows it either, which is what
            // setting a policy leaves a resource in.
            None => format!(
                "Action denied by stack policy: No statement allows [{}] for resource \
                 [{RESOURCE_PREFIX}{}];",
                self.action, self.logical_id
            ),
        }
    }

    /// The transitions the denied resource goes through: the update of it opened, and then
    /// failed carrying the policy's verdict. It keeps the identifier it has, nothing having
    /// been done to it.
    pub fn transitions(&self) -> Vec<ResourceTransition> {
        let opening = ResourceTransition {
            logical_id: self.logical_id.clone(),
            resource_type: self.resource_type.clone(),
            physical_resource_id: self.physical_resource_id.clone(),
            status: resource_status::UPDATE_IN_PROGRESS.to_string(),
            reason: None,
            resource_properties: None,
        };
        let failed = ResourceTransition {
            status: resource_status::UPDATE_FAILED.to_string(),
            reason: Some(self.reason()),
            ..opening.clone()
        };
        vec![opening, failed]
    }
}

/// The FIRST planned action `policy_body` does not permit, or `None` when it permits every
/// one of them.
///
/// The plan is walked in its own order, so the resource reported is the one the update
/// would have reached first — which is the resource a client's event stream names.
///
/// A body that is not a policy DOCUMENT at all — one that is not JSON, or that carries no
/// `Statement` element — governs nothing: such a body is one `SetStackPolicy` would not have
/// accepted, and no update is failed on account of a document nobody could read. A document
/// that IS one and states nothing about an action is a different matter: that action is
/// denied, which is what setting a stack policy leaves a resource in.
pub fn denied(policy_body: &str, planned: &[PlannedAction]) -> Option<Denial> {
    if !is_a_policy_document(policy_body) {
        return None;
    }
    // Read ONCE for the whole plan, since every action is decided against the same
    // document.
    let document = PreparedDocument::read(policy_body);
    planned
        .iter()
        .find_map(|action| verdict(&document, action))
}

/// Whether `policy_body` is a document this service can evaluate as a stack policy: JSON
/// carrying the `Statement` element the grammar is written in.
///
/// Read with `serde_json`, the one JSON reader of this tree, and read for its SHAPE alone —
/// what the statements say is [`::service_common::iam_authorization`]'s to decide.
fn is_a_policy_document(policy_body: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(policy_body)
        .is_ok_and(|document| !document[STATEMENT].is_null())
}

/// The verdict on ONE planned action: `Some(denial)` when the policy does not permit it.
///
/// The decision is [`::service_common::iam_authorization`]'s, over the document as a
/// RESOURCE-attached policy: a stack policy's statements name the `Principal` the grammar
/// requires them to (the wildcard) and the `Resource` they protect, which is that flavour's
/// shape, and the action is the request's. The one condition key such a statement may carry
/// travels as the request's own context, and is declared as modelled so a statement
/// conditioned on it is decided rather than withheld.
///
/// `Decision::NotStated` is this service's own reading: no statement about the action means
/// no permission for it, so the action is denied with no statement to name.
fn verdict(document: &PreparedDocument, action: &PlannedAction) -> Option<Denial> {
    let resource = format!("{RESOURCE_PREFIX}{}", action.logical_id);
    let context = [(
        RESOURCE_TYPE_KEY.to_string(),
        action.resource_type.clone(),
    )];
    let request = Request::new(action.action, &resource, &[])
        .with_context(&context)
        .with_published_keys(PUBLISHED_CONDITION_KEYS);
    let outcome = resource_outcome_prepared(document, &request);
    let denial = |statement: Option<usize>| Denial {
        statement,
        action: action.action,
        logical_id: action.logical_id.clone(),
        resource_type: action.resource_type.clone(),
        physical_resource_id: action.physical_resource_id.clone(),
    };
    match outcome.decision {
        Decision::Allowed => None,
        // The statements a denial was reached from are the DENYING ones, in document
        // order, so the first of them is the statement the refusal names.
        Decision::Denied => Some(denial(
            outcome.matched.first().map(|matched| matched.statement),
        )),
        Decision::NotStated => Some(denial(None)),
    }
}
