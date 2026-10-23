// SPDX-License-Identifier: CC0-1.0

//! Whether the EXECUTION ROLE an experiment template names may perform the API calls its
//! actions make — the question FIS answers when it runs an action under that role.
//!
//! FIS "runs the actions of an experiment under the IAM role you specify in the experiment
//! template", so an action whose API calls that role is not allowed to make cannot be
//! carried out: the experiment reaches the terminal status `failed`, and its state says so
//! in the vocabulary of an insufficient-privileges failure rather than in the vocabulary
//! of any other failure. That attribution is what a client branches on, so it is computed
//! here from PERSISTED state — the account's own IAM store in the shared engine, the very
//! records `CreateRole`, `PutRolePolicy` and `AttachRolePolicy` wrote — and never from the
//! request alone.
//!
//! Which IAM actions one FIS action needs is the catalog's
//! ([`ActionCatalogEntry::permissions`](super::ActionCatalogEntry::permissions)); the
//! DECISION those documents imply is not made here either — it is
//! [`::service_common::iam_authorization`], the ONE shared evaluation of IAM policy
//! documents against `(action, resource)`, so this service and every other one handed the
//! same documents answer alike. What lives here is the IAM-STORE LOOKUP that is specific
//! to this gate ([`role_documents`]: which documents are in force on an experiment's
//! execution role, read while the IAM guard is held) and the DECIDING walk over them
//! ([`permission_failure`], which holds no IAM guard at all). The same lookup, near
//! enough, lives in `service-com-amazonaws-transfer-transferservice`'s and
//! `service-com-amazonaws-rum-rum`'s own authorization modules; lifting it beside the
//! shared evaluator would make `services/common` depend on the IAM service crate, which is
//! a change to every service rather than to this one.
//!
//! The emulator refuses only what it can SHOW. Four readings follow from that, each the
//! one CloudWatch RUM's and Transfer Family's gates take of the same store:
//!
//! * a `roleArn` the account's IAM store holds NO role for is evidence of nothing (the
//!   role may belong to another account, or to a client that never told this emulator
//!   about it), so the run goes ahead;
//! * an AWS-MANAGED policy attached to the role is a document AWS owns and this account
//!   cannot read, so it might be the one that carries the grant: the run goes ahead;
//! * a CUSTOMER-managed policy the account does not hold grants nothing — the account IS
//!   the authority on its own policies;
//! * a role, or an action, whose documents or resources outrun the bounds below is one
//!   this gate does not finish reading, so nothing is shown about it either and the run
//!   goes ahead.
//!
//! # What this costs, and why that is a constant
//!
//! This walk runs inside `StartExperiment`, so its cost is paid per call by whoever calls
//! it. Every factor of it is bounded by a constant rather than by the size of what a
//! caller stored: the actions of a template by
//! [`MAX_ACTIONS_PER_TEMPLATE`](super::MAX_ACTIONS_PER_TEMPLATE) (20), the IAM actions one
//! of them needs by its catalog entry, the resources one action is decided against by
//! [`MAX_ACTION_RESOURCES`], the documents in force on a role by [`MAX_ROLE_DOCUMENTS`],
//! and the decisions one start spends altogether by [`MAX_PERMISSION_DECISIONS`]. Each
//! document is READ ONCE per start
//! ([`::service_common::iam_authorization::prepare_documents`]) rather than once per
//! decision, so a decision is a walk over statements already parsed; and the first grant
//! found ends the walk over an action's resources, because a gate that refuses only what
//! it can show has nothing left to look for once it has found the grant.

use std::collections::{BTreeMap, HashMap};

use ::service_common::iam_authorization::{self as iam_policy, Decision, PreparedDocument};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use super::{action_catalog_entry, ExperimentFailure, IamState, FIS_ACTION_ID_CONDITION_KEY};
use crate::ops::types::{ExperimentTemplateAction, ExperimentTemplateTarget};

/// The `Resource` an action that names NO resource ARN at all is decided against.
///
/// The empty value is what expresses "no resource" in the shared evaluator's terms: IAM's
/// wildcard matching lets only a statement covering every resource (`"*"`) match it, while
/// a statement scoped to one resource cannot decide such a request. The same value
/// Transfer Family's gate uses for a request that addresses no nameable ARN.
const NO_RESOURCE: &str = "";

/// The separator FIS documents for an action parameter that carries several ARNs: the
/// `alarmArns` of `aws:cloudwatch:assert-alarm-state` is "the Amazon Resource Names (ARNs)
/// of the alarms, separated by commas".
const PARAMETER_LIST_SEPARATOR: char = ',';

/// The most resources ONE action's permission is decided against.
///
/// In the spirit of [`MAX_TARGET_RESOURCE_ARNS`](super::MAX_TARGET_RESOURCE_ARNS), the
/// model's own ceiling of 5 on the resources one TARGET names: an action's `parameters` are
/// stored exactly as the request sent them and nothing bounds that map, so this is what
/// keeps the decision's cost a constant instead of a function of the stored request's
/// size. The candidates are taken in a stable order (see [`action_resources`]), so which
/// ones a capped action is decided against does not depend on a map's iteration order.
pub const MAX_ACTION_RESOURCES: usize = 8;

/// The most policy documents this gate reads for ONE role.
///
/// A role carries its inline policies plus at most the IAM store's own ceiling of attached
/// managed policies. A role whose documents outnumber this is one the gate does not finish
/// reading, and an unread document might be the one that carries the grant — so nothing is
/// shown about such a role and its run goes ahead (see the module docs).
pub const MAX_ROLE_DOCUMENTS: usize = 32;

/// The most `(permission, resource)` decisions ONE start spends on this gate.
///
/// The product of the factors above is already bounded, and this bounds the product
/// itself, so no template can make a start pay more than a fixed amount of this work. A
/// start that spends the budget stops deciding: the actions it did not reach are ones
/// nothing was shown about, which is the same reading every other "cannot be shown" case
/// here takes.
pub const MAX_PERMISSION_DECISIONS: usize = 32;

/// The identity-based policy documents in force on the role `role_arn` names: its inline
/// role policies plus the documents of the customer-managed policies attached to it.
///
/// Called with the IAM guard held, and it CLONES what it reads, so the deciding walk
/// ([`permission_failure`]) holds no IAM guard: the store is not locked while this gate
/// thinks.
///
/// `None` when nothing can be SHOWN about the role's permissions — the store holds no such
/// role, one of its attachments is an AWS-managed policy whose document this account does
/// not hold, or it carries more documents than [`MAX_ROLE_DOCUMENTS`] (see the module
/// docs) — which is what makes the caller refuse nothing.
pub fn role_documents(role_arn: &str, iam: &IamState) -> Option<Vec<String>> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let role_key = iam::name_key(&role.role_name);
    let mut documents: Vec<String> = iam
        .role_inline_policies
        .get(&role_key)
        .into_iter()
        .flat_map(|policies| policies.values())
        .map(|policy| policy.policy_document.clone())
        .collect();
    for arn in iam
        .role_attached_policies
        .get(&role_key)
        .into_iter()
        .flatten()
    {
        match default_policy_document(iam, arn) {
            Some(document) => documents.push(document),
            // An AWS-managed policy might be the one that carries the grant, and this
            // account cannot read it: nothing can be shown about this role.
            None if is_aws_managed_policy(arn) => return None,
            None => {}
        }
    }
    // More documents than this gate reads: an unread one might carry the grant.
    if documents.len() > MAX_ROLE_DOCUMENTS {
        return None;
    }
    Some(documents)
}

/// `Some(failure)` when `documents` — the policies in force on the role `role_arn` names,
/// as [`role_documents`] read them — show that role is not allowed to perform the API
/// calls one of `actions` makes, and `None` when they show nothing of the kind.
///
/// The actions are examined in NAME order, so an experiment whose template has two
/// under-privileged actions fails on the same one at every start rather than on whichever
/// one a map iteration reached first. The work is bounded by the constants above.
pub fn permission_failure(
    role_arn: &str,
    actions: &HashMap<String, ExperimentTemplateAction>,
    targets: &HashMap<String, ExperimentTemplateTarget>,
    documents: &[String],
) -> Option<ExperimentFailure> {
    // Read once per start, not once per decision.
    let documents = iam_policy::prepare_documents(documents);
    let mut budget = MAX_PERMISSION_DECISIONS;
    let by_name: BTreeMap<&str, &ExperimentTemplateAction> = actions
        .iter()
        .map(|(name, action)| (name.as_str(), action))
        .collect();
    for (name, action) in by_name {
        let action_id = action.action_id.as_deref().unwrap_or_default();
        // An action id this emulator's catalog does not hold states no permissions, so
        // there is nothing to show about it: it is not failed for.
        let Some(entry) = action_catalog_entry(action_id) else {
            continue;
        };
        if entry.permissions.is_empty() {
            continue;
        }
        let resources = action_resources(action, targets);
        // The condition context the decision knows: the FIS action id, which is the key
        // the documented `ec2:InjectApiError` grant is scoped by
        // (`FIS_ACTION_ID_CONDITION_KEY`). A policy written the way the FIS actions
        // reference writes it is therefore DECIDED — it grants the action it names and
        // refuses the one it does not — instead of being undecidable and granting nothing.
        let context = [(
            FIS_ACTION_ID_CONDITION_KEY.to_string(),
            action_id.to_string(),
        )];
        for permission in entry.permissions {
            match role_may_perform(&documents, permission, &resources, &context, &mut budget) {
                // Nothing was shown against this permission, or the budget ran out before
                // anything could be: either way this gate shows nothing here.
                Some(true) | None => {}
                Some(false) => {
                    return Some(ExperimentFailure::insufficient_privileges(
                        role_arn,
                        name,
                        action_id,
                        permission,
                    ))
                }
            }
        }
    }
    None
}

/// Whether `documents` allow `permission` on at least one of the resources the action
/// names; `None` when `budget` ran out before the question could be decided.
///
/// The rule as implemented: the candidates are walked in the stable order
/// [`action_resources`] built them in, and the walk ends at the FIRST candidate whose
/// decision is not "nothing stated" — a `Deny` there refuses the action, an `Allow` there
/// is the grant. So an action FIS performs on several resources is allowed when the role
/// may perform it on a resource it actually reaches, and a role whose policies state
/// nothing about the action at all is not allowed to perform it. Ending at the first
/// decision is what keeps the walk short in the common case, and it cannot turn a refusal
/// into a grant that the documents do not state: this gate refuses only what they SHOW,
/// and a grant is the end of showing anything. The consequence to know is that a document
/// set which allows the permission on one resource the action reaches and denies it on
/// another is read as a grant, by the candidate that comes first.
///
/// `context` carries the condition keys this decision knows (the FIS action id), so a
/// statement scoped by one of them is decided rather than undecidable.
fn role_may_perform(
    documents: &[PreparedDocument],
    permission: &str,
    resources: &[String],
    context: &[(String, String)],
    budget: &mut usize,
) -> Option<bool> {
    let candidates: Vec<&str> = if resources.is_empty() {
        vec![NO_RESOURCE]
    } else {
        resources.iter().map(String::as_str).collect()
    };
    for resource in candidates {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let request = iam_policy::Request::new(permission, resource, &[]).with_context(context);
        match iam_policy::identity_decision_prepared(documents, &request) {
            Decision::Denied => return Some(false),
            Decision::Allowed => return Some(true),
            Decision::NotStated => {}
        }
    }
    Some(false)
}

/// The resource ARNs one action NAMES, at most [`MAX_ACTION_RESOURCES`] of them: the
/// resources of the targets it refers to, and the ARNs among its own parameter values.
///
/// An action acts on its targets ("the targets for the action", by the name the template
/// gave them) and, for an action that takes resources as parameters rather than as targets
/// (`aws:cloudwatch:assert-alarm-state`'s `alarmArns`), on the resources those parameters
/// name. A parameter value that is not an ARN names no resource and contributes none.
///
/// The walk is in a STABLE order — the targets the action names and then its parameters,
/// both by key — so the candidates a capped action is decided against are the same at
/// every start rather than whichever ones a `HashMap` happened to yield first.
fn action_resources(
    action: &ExperimentTemplateAction,
    targets: &HashMap<String, ExperimentTemplateTarget>,
) -> Vec<String> {
    let mut resources: Vec<String> = Vec::new();
    let named_targets: BTreeMap<&str, &str> = action
        .targets
        .iter()
        .flatten()
        .map(|(key, name)| (key.as_str(), name.as_str()))
        .collect();
    for target_name in named_targets.values() {
        if let Some(target) = targets.get(*target_name) {
            for arn in target.resource_arns.iter().flatten() {
                if push_bounded(&mut resources, arn.clone()) {
                    return resources;
                }
            }
        }
    }
    let parameters: BTreeMap<&str, &str> = action
        .parameters
        .iter()
        .flatten()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    for value in parameters.values() {
        for candidate in value.split(PARAMETER_LIST_SEPARATOR).map(str::trim) {
            if ::service_common::arn::parse(candidate).is_none() {
                continue;
            }
            if push_bounded(&mut resources, candidate.to_string()) {
                return resources;
            }
        }
    }
    resources
}

/// Push `resource` and report whether the list is now FULL — the one place
/// [`MAX_ACTION_RESOURCES`] is applied, so no caller can walk past it.
fn push_bounded(resources: &mut Vec<String>, resource: String) -> bool {
    resources.push(resource);
    resources.len() >= MAX_ACTION_RESOURCES
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's own
/// `aws`, so AWS owns the document and this account's store holds none for it. Read
/// through IAM's OWN ARN parser and its own constant, so this service and IAM agree on
/// what an AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The default version's document of the managed policy `arn` names, or `None` when the
/// account holds no such policy or no document for its default version.
///
/// The version is the one `Policy$DefaultVersionId` names, falling back to the version
/// that carries `PolicyVersion$IsDefaultVersion` — the two spellings of "the version in
/// force" the IAM store can hold.
fn default_policy_document(iam: &IamState, arn: &str) -> Option<String> {
    let managed = iam.policy_by_arn(arn)?;
    managed
        .policy
        .default_version_id
        .as_deref()
        .and_then(|default| managed.versions.get(default))
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}
