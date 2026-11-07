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
//! What a denial REPORTS is the denying statement itself — its position in the stored
//! document and the patterns it forbids ([`Denial::reason`]) — because the only thing that
//! reason exists to say is which statement blocked the update and what it does not allow.
//!
//! A stored policy is also validated against the template an update submits
//! ([`unknown_logical_id`]): a statement naming a logical id that template does not declare
//! could never be evaluated, so CloudFormation refuses the update before it deploys
//! anything.
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

/// The most bytes of policy document a stack policy named BY REFERENCE may weigh: "The URL
/// must point to a policy (maximum size: 16 KB)" — the same ceiling the inline
/// `StackPolicyBody` carries (`@length(max: 16384)`), since the reference says WHERE the
/// document is and not what it may be.
pub const POLICY_BODY_MAX_BYTES: usize = 16_384;

/// `Err(message)` for a policy document past that ceiling, naming the member the document
/// would have been submitted on and the constraint it failed — the wording this service
/// holds every caller-sized document to ([`super::template_body_too_long`] is the same
/// sentence for a template body).
///
/// The member is named as a VIOLATION PATH spells it, in lowerCamel, which is the spelling
/// every constraint message of this surface carries.
fn policy_body_too_long(body_member: &str) -> String {
    let path = violation_path(body_member);
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must have length less than or equal to {POLICY_BODY_MAX_BYTES}"
    )
}

/// One modelled member name as a VIOLATION PATH spells it: lowerCamel, which is the spelling
/// every constraint message of this surface carries (`StackPolicyBody` → `stackPolicyBody`,
/// beside this crate's own `templateBody`).
fn violation_path(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The stack policy document a `…URL` member addresses, read from this emulator's own Amazon
/// S3 — the same read a template submitted by reference goes through
/// ([`super::template_url`]).
///
/// A stack policy reaches CloudFormation one of two ways, exactly as a template does: inline,
/// as the request's own policy BODY, or by reference, as a URL naming "a policy (maximum
/// size: 16 KB) located in an Amazon S3 bucket in the same Amazon Web Services Region as the
/// stack". The reference says WHERE the document is, not what it means, so the object it
/// addresses becomes the body the operation acts on and is then held to every rule a
/// submitted body is held to.
///
/// `url_member` is the member the reference was given on and `body_member` the one its
/// document would have been given on: a refusal is about the REQUEST's own member, so both
/// the unreadable-reference sentences and the ceiling name the member that carried it.
///
/// `Err(message)` is the `ValidationError` message the operation answers.
pub async fn fetch(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    url_member: &str,
    body_member: &str,
    url: &str,
) -> Result<String, String> {
    let object = super::template_url::object_of(url_member, url)?;
    super::template_url::read(
        ctx,
        region,
        &object,
        POLICY_BODY_MAX_BYTES,
        &policy_body_too_long(body_member),
    )
    .await
}

/// `Err(message)` for a request that submits a stack policy BOTH ways: "You can specify
/// either the StackPolicyBody or the StackPolicyURL parameter, but not both" — the rule every
/// operation that takes a policy is held to, worded once for the pair of members it is about.
pub fn one_policy_source(
    body_member: &str,
    url_member: &str,
    body: Option<&str>,
    url: Option<&str>,
) -> Result<(), String> {
    if body.is_some() && url.is_some() {
        return Err(format!(
            "You cannot specify both {body_member} and {url_member}"
        ));
    }
    Ok(())
}

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

/// The members one statement spells what it governs with, and the `Not…` spellings of each:
/// what the statement states about an action, and about the resource it is performed on.
const ACTION: &str = "Action";
const NOT_ACTION: &str = "NotAction";
const RESOURCE: &str = "Resource";
const NOT_RESOURCE: &str = "NotResource";

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

/// What the statement that denied an action FORBIDS, as that statement itself spells it:
/// the `Action` and `Resource` patterns it carries.
///
/// These are the statement's own patterns and not the change that was attempted — a
/// statement denying `Update:*` on `*` forbids exactly that, whichever resource an update
/// happened to reach — which is what a denial names ([`Denial::reason`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyingStatement {
    /// The statement's `Action`, as written: one pattern, or every pattern of a list in the
    /// order the statement lists them.
    pub action: String,
    /// The statement's `Resource`, as written, read the same way.
    pub resource: String,
}

/// An update action the stack policy in force does not permit: which resource, which
/// action, and the statement that denied it where one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The index of the `Statement` that denies the action, counted from zero as the
    /// evaluator reports it, or `None` where no statement allows it.
    pub statement: Option<usize>,
    /// What that statement forbids, read back out of the stored document
    /// ([`statement_patterns`]); `None` where no statement denied the action.
    pub denying: Option<DenyingStatement>,
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
    /// What the denied resource's own `UPDATE_FAILED` event reports: WHICH statement denied
    /// the action, and what that statement forbids.
    ///
    /// The index is the statement's position in the stored document counted from ONE, which
    /// is how a policy document numbers its statements to a reader, and the action and the
    /// resource are that statement's own patterns rather than the change that was attempted:
    /// the reason exists to say which statement blocked the update and what it forbids, which
    /// the attempted change cannot say.
    pub fn reason(&self) -> String {
        match (self.statement, &self.denying) {
            (Some(index), Some(denying)) => format!(
                "Action denied by stack policy: Statement [#{}] does not allow \
                 [{}] for resource [{}];",
                index + 1,
                denying.action,
                denying.resource
            ),
            // A statement denied the action and the stored document no longer reads back as
            // one (it is read twice: once by the evaluator, once for its patterns), so the
            // statement is named by its position alone and what was asked of it stands in
            // for what it forbids.
            (Some(index), None) => format!(
                "Action denied by stack policy: Statement [#{}] does not allow \
                 [{}] for resource [{RESOURCE_PREFIX}{}];",
                index + 1,
                self.action,
                self.logical_id
            ),
            // No statement denies the action and none allows it either, which is what
            // setting a policy leaves a resource in.
            (None, _) => format!(
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
    let mut denial = planned
        .iter()
        .find_map(|action| verdict(&document, action))?;
    // What the denying statement forbids is read out of the stored document itself, so the
    // reason states that statement's own `Action` and `Resource` rather than the change that
    // was attempted.
    denial.denying = denial
        .statement
        .and_then(|index| statement_patterns(policy_body, index));
    Some(denial)
}

/// The `Action` and `Resource` the statement at `index` of `policy_body` carries, or `None`
/// where the document holds no statement there.
///
/// Read with `serde_json`, the one JSON reader of this tree, from the same document the
/// evaluator decided against; `NotAction`/`NotResource` are read where a statement spells its
/// patterns that way, since that is then what the statement states about the action. A list
/// of patterns is rendered in the order the statement lists them, which is the order a reader
/// of the document sees.
fn statement_patterns(policy_body: &str, index: usize) -> Option<DenyingStatement> {
    let document: ::serde_json::Value = ::serde_json::from_str(policy_body).ok()?;
    let statements = &document[STATEMENT];
    let statement = match statements {
        ::serde_json::Value::Array(statements) => statements.get(index)?,
        // A document whose `Statement` is one object is one statement, at index zero.
        other if index == 0 => other,
        _ => return None,
    };
    Some(DenyingStatement {
        action: patterns_of(statement, ACTION, NOT_ACTION),
        resource: patterns_of(statement, RESOURCE, NOT_RESOURCE),
    })
}

/// One statement member's patterns as the statement writes them: the member itself, else its
/// `Not…` spelling, rendered as the one pattern it names or as every pattern of a list in
/// the statement's own order. Empty where the statement names neither.
fn patterns_of(
    statement: &::serde_json::Value,
    member: &str,
    negated: &str,
) -> String {
    let value = match &statement[member] {
        ::serde_json::Value::Null => &statement[negated],
        stated => stated,
    };
    match value {
        ::serde_json::Value::String(pattern) => pattern.clone(),
        ::serde_json::Value::Array(patterns) => patterns
            .iter()
            .filter_map(|pattern| pattern.as_str())
            .collect::<Vec<&str>>()
            .join(", "),
        _ => String::new(),
    }
}

/// `Err(message)` for a STORED stack policy that names a logical id the submitted template
/// does not declare, which is what CloudFormation validates an update's existing policy
/// against before it deploys anything.
///
/// "Stack policies can only be applied to logical ids referenced in the template": a
/// statement protecting a resource no longer in the template could never be evaluated, so
/// the update is refused outright and the stack keeps the status — and the policy — it has.
/// A pattern is a logical id only where it names ONE
/// ([`RESOURCE_PREFIX`] followed by text carrying no wildcard); `*`, `LogicalResourceId/*`
/// and every other pattern match whatever the template declares, so none of them can name
/// something it does not.
///
/// A body that is no policy document governs nothing ([`is_a_policy_document`]), exactly as
/// it governs no action, so nothing is refused on account of one.
pub fn unknown_logical_id(policy_body: &str, declared: &[String]) -> Result<(), String> {
    if !is_a_policy_document(policy_body) {
        return Ok(());
    }
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy_body) else {
        return Ok(());
    };
    let statements = match &document[STATEMENT] {
        ::serde_json::Value::Array(statements) => statements.clone(),
        other => vec![other.clone()],
    };
    for statement in &statements {
        for member in [RESOURCE, NOT_RESOURCE] {
            let patterns = match &statement[member] {
                ::serde_json::Value::String(pattern) => vec![pattern.clone()],
                ::serde_json::Value::Array(patterns) => patterns
                    .iter()
                    .filter_map(|pattern| pattern.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            };
            for pattern in patterns {
                let Some(logical_id) = pattern.strip_prefix(RESOURCE_PREFIX) else {
                    continue;
                };
                if logical_id.contains('*') || logical_id.contains('?') {
                    continue;
                }
                if !declared.iter().any(|declared| declared == logical_id) {
                    return Err(unknown_logical_id_message(&pattern));
                }
            }
        }
    }
    Ok(())
}

/// How an update whose stored stack policy protects a logical id the submitted template does
/// not declare is refused: fixed text apart from the pattern the statement named.
fn unknown_logical_id_message(pattern: &str) -> String {
    format!(
        "Error validating existing stack policy: Unknown logical id '{pattern}' in \
         statement {{}} - stack policies can only be applied to logical ids referenced in \
         the template"
    )
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
        // Filled by [`denied`], which has the document the patterns are read out of.
        denying: None,
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
