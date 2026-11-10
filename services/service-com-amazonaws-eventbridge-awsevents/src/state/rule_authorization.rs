// SPDX-License-Identifier: CC0-1.0

//! Whether the policies in force let the calling principal CHANGE a rule — the
//! authorization pass every rule mutator of this service takes before it writes anything.
//!
//! Changing a rule is the action `events:<OperationName>` on that rule's own ARN
//! ([`rule_action`], built on this service's `arnNamespace`), so AWS decides it the way it
//! decides any call on a resource: against the identity-based policies in force on the
//! principal the request's credential speaks for, at the instant of the call. A rule carries
//! no resource-based policy of its own — `PutPermission` installs one on an event BUS
//! ([`super::BusPolicy`]) and on nothing else — so the identity half is the whole decision
//! here.
//!
//! The piece this module exists for is the CONDITION CONTEXT. A statement about a rule can
//! be scoped by the tags the rule HOLDS, through the global condition key family
//! `aws:ResourceTag/<tag-key>` ("compares the tag key-value pair that you specify in the
//! policy with the key-value pair attached to the resource"), which is the primary way an
//! account scopes who may touch which rules. The values of that family are a fact of the
//! TARGET rule rather than of the request, so they are read off the stored rule
//! ([`rule_condition_context`]) and the decision follows that rule's tags: the identical call
//! against a rule carrying another tag value is refused, and retagging the rule changes the
//! answer with no change to the call.
//!
//! WHO the caller is, and which documents are in force on it, is
//! [`super::authorization::request_caller`] — the same resolution the publication gate takes,
//! so one credential speaks for one principal whatever it asks for. The decision itself is
//! [`::service_common::iam_authorization`], the tree's ONE evaluation of policy documents
//! against `(action, resource, condition context)`.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the call in an identity-based document
//! refuses it whatever grants it; otherwise the call needs an `Allow` about it in those
//! documents; with neither it is refused, which is IAM's implicit deny, and the rule is left
//! exactly as it was.
//!
//! The two callers this gate does not bound are the two
//! [`super::authorization`] names, for the same reasons: the account's own published
//! credential, which the IAM store holds no policies for and which has full control of the
//! account's own rules, and a principal carrying an attachment whose document this emulator
//! cannot read, where refusing would deny access AWS permits.
//!
//! ## The condition keys this gate does not publish yet
//!
//! Only the resource-tag family is published. AWS also publishes the REQUEST-tag families for
//! a call that carries tags of its own — `aws:RequestTag/<tag-key>` and `aws:TagKeys` — which
//! here would be the `Tags` a rule-creating `PutRule` or a `TagResource` carries and the
//! `TagKeys` an `UntagResource` names. A statement conditioned on one of those is therefore
//! left UNDECIDED, which never grants and never weakens a deny
//! ([`::service_common::iam_authorization`]): such a grant refuses a call AWS would serve,
//! which is the fail-closed direction, and reading the request's own tags onto the context is
//! what closes it.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::authorization::Caller;
use super::*;

/// The IAM action namespace this service's actions are published under: an action is
/// `events:<OperationName>`, this service's `arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ACTION_NAMESPACE: &str = ARN_NAMESPACE;

/// The condition key family carrying the tags the ADDRESSED resource holds:
/// "`aws:ResourceTag/tag-key` — compares the tag key-value pair that you specify in the
/// policy with the key-value pair attached to the resource".
pub const RESOURCE_TAG_PREFIX: &str = "aws:ResourceTag/";

/// The condition keys a call on a rule DECLARES it models: the whole
/// [`RESOURCE_TAG_PREFIX`] family, because the addressed rule's tag set is known in full —
/// which is what lets a clause written with IAM's `IfExists` suffix, or a `Null` clause, be
/// answered for a tag key the rule does not carry. A key outside this family leaves its
/// statement undecided, which never grants and never weakens a deny.
const RULE_CONDITION_KEYS: [&str; 1] = [RESOURCE_TAG_PREFIX];

/// The IAM action the operation `operation` performs — `events:DisableRule` for
/// `DisableRule`, and so on for every operation of this service.
#[must_use]
pub fn rule_action(operation: &str) -> String {
    format!("{ACTION_NAMESPACE}:{operation}")
}

/// The condition context a call on a rule carrying `tags` is decided with: one
/// `aws:ResourceTag/<key>` pair per tag the rule holds.
#[must_use]
pub fn rule_condition_context(tags: &[Tag]) -> Vec<(String, String)> {
    tags.iter()
        .map(|tag| (format!("{RESOURCE_TAG_PREFIX}{}", tag.key), tag.value.clone()))
        .collect()
}

/// The `AccessDeniedException` message the call `operation` makes on the rule `rule_name` of
/// the bus `bus_name` in `region` earns when the policies in force do not permit it, or
/// `None` when they do.
///
/// The rule is read from `store` for the two facts the decision is made from: the ARN IAM
/// judges the action against, and the tags that give [`RESOURCE_TAG_PREFIX`] its values. A
/// name this store holds no rule under is judged against the ARN such a rule WOULD carry
/// ([`rule_arn`], the one place this service mints one) and an empty tag set, because IAM
/// decides before the service looks at the resource — so a caller holding no grant reads
/// that it is not authorized whether or not the rule exists, and a caller holding one goes
/// on to the operation's own answer about a resource that is missing.
///
/// Decided with no store of another service reached: `caller` is resolved BEFORE this
/// service's store is locked ([`super::authorization::request_caller`]), and everything
/// below is a pure evaluation of the documents it carries.
#[must_use]
pub fn rule_mutation_denial(
    store: &ComAmazonawsEventbridgeAwseventsState,
    caller: &Caller,
    operation: &str,
    region: &str,
    bus_name: &str,
    rule_name: &str,
) -> Option<String> {
    let stored = store.rule(region, bus_name, rule_name);
    let rule_arn = stored.map_or_else(
        || rule_arn(region, bus_name, rule_name),
        |rule| rule.arn.clone(),
    );
    let tags = stored.map(|rule| rule.tags.as_slice()).unwrap_or_default();
    rule_action_denial(caller, &rule_action(operation), &rule_arn, tags)
}

/// [`rule_mutation_denial`] for a call that addressed its rule by ARN rather than by name:
/// the rule the tagging operations resolved ([`parse_tagged_resource`]).
///
/// A call that addressed an event BUS is outside this gate — a bus is authorized against its
/// own resource policy as well as the caller's documents, which is a decision of its own —
/// so such a call earns no refusal here.
#[must_use]
pub fn tagged_rule_denial(
    store: &ComAmazonawsEventbridgeAwseventsState,
    caller: &Caller,
    operation: &str,
    resource: &TaggedResource,
) -> Option<String> {
    let TaggedResource::Rule {
        region,
        event_bus_name,
        name,
    } = resource
    else {
        return None;
    };
    rule_mutation_denial(store, caller, operation, region, event_bus_name, name)
}

/// The refusal the identity-based documents in force on `caller` state against `action` on
/// `rule_arn`, evaluated with the rule's own `tags`, or `None` when they permit it.
///
/// The module docs' rule, in IAM's own order.
#[must_use]
pub fn rule_action_denial(
    caller: &Caller,
    action: &str,
    rule_arn: &str,
    tags: &[Tag],
) -> Option<String> {
    let context = rule_condition_context(tags);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action, the resource and the context; the principal spellings travel with the
    // request because that is what such an element would be matched against.
    let request = iam::Request::new(action, rule_arn, &caller.principals)
        .with_context(&context)
        .with_published_keys(&RULE_CONDITION_KEYS);
    match iam::identity_decision(&caller.documents, &request) {
        // An explicit deny in a document that CAN be read refuses the call whatever else
        // the principal carries.
        Decision::Denied => Some(not_authorized(
            &caller.arn,
            action,
            rule_arn,
            "with an explicit deny in an identity-based policy",
        )),
        Decision::Allowed => None,
        // The two callers this gate does not bound (see the module docs).
        Decision::NotStated if caller.account_credential || caller.unreadable_attachment => None,
        Decision::NotStated => Some(not_authorized(
            &caller.arn,
            action,
            rule_arn,
            &format!("because no identity-based policy allows the {action} action"),
        )),
    }
}
