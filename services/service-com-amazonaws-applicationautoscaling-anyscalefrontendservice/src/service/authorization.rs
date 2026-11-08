// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step taken ahead of every operation, before any handler reads or writes
//! anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the resource,
//! so a caller whose identity-based policies do not grant the action is told it is not
//! authorized and the call performs nothing. It is a fact of the CREDENTIAL the request
//! presented and the policies in force on the principal it speaks for, not of the store, so
//! it is decided once here rather than in each of this service's handlers.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for (`sts_store::resolve_caller`, the same place `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for
//!    the credentials it signed with), else the IAM user that owns the long-term key, else
//!    the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes effect on
//!    the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource, condition context)`, so this service
//!    and every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls to
//!    IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not.
//!
//! ## What the decision is made about
//!
//! The ACTION is `application-autoscaling:<OperationName>`, this service's `arnNamespace`
//! ("Actions defined by AWS Application Auto Scaling", IAM Service Authorization Reference)
//! in front of the operation the request named. A registration that carries `Tags` asks for
//! a second action as well: "to use tags, you must also have permission to use the
//! `application-autoscaling:TagResource` action", so both are required and either one
//! withheld refuses the call.
//!
//! The RESOURCE is the scalable target the request addresses: its own ARN where this service
//! holds that target, and otherwise every scalable target of the Region
//! ([`every_scalable_target`]) — a registration names a target that does not exist yet, and
//! the ARN its create will mint is no value a policy could have been written against.
//!
//! The CONDITION CONTEXT is what the request carries that this service publishes keys for:
//! `application-autoscaling:service-namespace` and
//! `application-autoscaling:scalable-dimension`, the two condition keys this service
//! publishes, plus the global tag keys — `aws:RequestTag/<key>` and `aws:TagKeys` for the
//! tags the request carries, and `aws:ResourceTag/<key>` for the tags the addressed target
//! holds. That is what lets a grant scoped to one dimension, or to a tagged target, be
//! decided here exactly as IAM decides it.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a permissions boundary the
//!   store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (A session the authenticating layer has
//! already refused never reaches a handler at all.)
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
//! session expiries the clock has already made due; that write is owed at the same instant
//! whether or not anybody called, which is why a REPORTING operation of this service may take
//! this decision too.

use std::time::SystemTime;

use ::serde_json::Value;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `application-autoscaling:<OperationName>`,
/// which is this service's `arnNamespace` (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "application-autoscaling";

/// The action a tag-carrying request needs BESIDE the operation's own, because tagging is a
/// permission of its own: `application-autoscaling:TagResource`.
const TAG_RESOURCE_ACTION: &str = "TagResource";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The condition keys this gate can answer for, whether or not the request carries a value
/// for each — the two this service publishes, and the three global tag families. A key
/// outside this set leaves its statement undecided, which never grants and never weakens a
/// deny.
const PUBLISHED_CONDITION_KEYS: &[&str] = &[
    "application-autoscaling:service-namespace",
    "application-autoscaling:scalable-dimension",
    "aws:RequestTag/",
    "aws:ResourceTag/",
    "aws:TagKeys",
];

/// An authorization refusal: who was refused, which action, on which resource, and which of
/// IAM's rules withheld the grant.
#[derive(Debug, Clone)]
pub struct Denied {
    /// The principal the request speaks for, as `sts:GetCallerIdentity` reports it.
    principal: String,
    /// The IAM action the request asked for.
    action: String,
    /// The ARN the request addressed.
    resource: String,
    /// Why the grant was withheld, as the refusal's closing clause words it.
    reason: String,
}

impl Denied {
    /// The refusal in IAM's own wording: the principal the credential speaks for, the action
    /// the request asked for, the resource it addressed and which of IAM's rules refused.
    /// Every value in it comes from the request and from the stored policies.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "User: {} is not authorized to perform: {} on resource: {} {}",
            self.principal, self.action, self.resource, self.reason
        )
    }
}

/// Whether IAM lets this request perform the action of `operation` — the operation's own
/// name, which is the routing key the request named.
///
/// `request` is the request's own document, read for the members the condition keys are
/// built from; a body that is no object carries none of them, which is the same as a request
/// that names neither a namespace nor a dimension.
///
/// `Ok(())` for a request IAM permits and for every caller this gate does not bound (see the
/// module docs); `Err` carries the refusal this service answers as `AccessDeniedException`.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    request: &Value,
) -> Result<(), Denied> {
    let caller = caller(engine, ctx);
    let addressed = addressed(engine, ctx, request);
    let context = condition_context(request, &addressed);
    // Every action the call needs: the operation's own, and — for a registration that
    // carries tags — the tagging permission such a call also requires.
    let mut actions = vec![format!("{ACTION_PREFIX}:{operation}")];
    if carries_tags(request) {
        actions.push(format!("{ACTION_PREFIX}:{TAG_RESOURCE_ACTION}"));
    }
    for action in actions {
        decide(&caller, &action, &addressed.resource, &context)?;
    }
    Ok(())
}

/// One action's decision, under IAM's own evaluation order.
fn decide(
    caller: &Caller,
    action: &str,
    resource: &str,
    context: &[(String, String)],
) -> Result<(), Denied> {
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no such
    // element, so the decision below rests on the action, the resource and the context.
    let request = iam::Request::new(action, resource, &caller.principals)
        .with_context(context)
        .with_published_keys(PUBLISHED_CONDITION_KEYS);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(caller.refused(
            action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(caller.refused(
            action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(caller.refused(action, resource, &reason));
    }
    Ok(())
}

/// The scalable target a request addresses, as the decision needs it.
#[derive(Debug, Clone, Default)]
struct Addressed {
    /// The ARN IAM judges the action against (see the module docs).
    resource: String,
    /// The tags the addressed target HOLDS, which are the values of `aws:ResourceTag/<key>`.
    resource_tags: Vec<(String, String)>,
}

/// Every scalable target of one Region, as IAM writes the collection an action that addresses
/// no single resource is judged against.
///
/// Built through the same ARN builder every scalable-target ARN this service mints comes
/// from, so a policy scoped to this account's targets in this Region covers it.
fn every_scalable_target(region: &str) -> String {
    ::emulator_server::emulator_arn(ACTION_PREFIX, region, "scalable-target/*")
}

/// The target the request addresses: its stored ARN and tags where this service holds that
/// target, and the Region's whole collection otherwise.
fn addressed<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    request: &Value,
) -> Addressed {
    let region = crate::state::request_region(ctx.parts());
    let mut addressed = Addressed {
        resource: every_scalable_target(&region),
        resource_tags: Vec::new(),
    };
    let (Some(service_namespace), Some(resource_id), Some(scalable_dimension)) = (
        member(request, "ServiceNamespace"),
        member(request, "ResourceId"),
        member(request, "ScalableDimension"),
    ) else {
        return addressed;
    };
    let key = crate::state::TargetKey {
        region,
        service_namespace: service_namespace.to_string(),
        resource_id: resource_id.to_string(),
        scalable_dimension: scalable_dimension.to_string(),
    };
    let Some(state) = crate::state::service_state(engine) else {
        return addressed;
    };
    // A READ of this service's own store, taken and released before the operation runs: it
    // changes nothing a later caller sees.
    let state = state.lock();
    if let Some(record) = state.targets.get(&key) {
        addressed.resource = record.arn.clone();
        addressed.resource_tags = record
            .tags
            .iter()
            .map(|(tag, value)| (tag.clone(), value.clone()))
            .collect();
    }
    addressed
}

/// The condition keys this request carries values for: the two this service publishes, read
/// off the request's own members, and the global tag keys — the request's tags, and the tags
/// the addressed target holds.
fn condition_context(request: &Value, addressed: &Addressed) -> Vec<(String, String)> {
    let mut context = Vec::new();
    if let Some(service_namespace) = member(request, "ServiceNamespace") {
        context.push((
            "application-autoscaling:service-namespace".to_string(),
            service_namespace.to_string(),
        ));
    }
    if let Some(scalable_dimension) = member(request, "ScalableDimension") {
        context.push((
            "application-autoscaling:scalable-dimension".to_string(),
            scalable_dimension.to_string(),
        ));
    }
    for (tag, value) in request_tags(request) {
        context.push((format!("aws:RequestTag/{tag}"), value));
        // `aws:TagKeys` carries one value per key the request names, which is what IAM's set
        // qualifiers compare against.
        context.push(("aws:TagKeys".to_string(), tag));
    }
    for (tag, value) in &addressed.resource_tags {
        context.push((format!("aws:ResourceTag/{tag}"), value.clone()));
    }
    context
}

/// The tags a request carries, as `(key, value)` pairs. `Tags` is a MAP on this service's
/// tag-carrying operations (`RegisterScalableTarget`, `TagResource`), so a body that states
/// one in any other shape carries no tag this gate can read.
fn request_tags(request: &Value) -> Vec<(String, String)> {
    request
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(tag, value)| {
                    (
                        tag.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the request asks for tags to be attached, which is what makes the tagging
/// permission part of what it needs.
fn carries_tags(request: &Value) -> bool {
    !request_tags(request).is_empty()
}

/// One string member of the request document, when it states a non-empty one.
fn member<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// This caller's refusal of one action on one resource.
    fn refused(&self, action: &str, resource: &str, reason: &str) -> Denied {
        Denied {
            principal: self.arn.clone(),
            action: action.to_string(),
            resource: resource.to_string(),
            reason: reason.to_string(),
        }
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The key
        // id it presented is NOT quoted back — this module echoes no value of the request.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the user's
    // policies rather than served as the account.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // The one reading of each principal form, shared with every other gate in the tree.
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults — asked here only to pin which CONTEXT this gate
    /// hands it. That the action is `application-autoscaling:<OperationName>`, and the whole
    /// decision end to end, is pinned over the wire by
    /// `tests/all/request_authorization.rs`.
    fn decision(document: &str, action: &str, resource: &str, context: &[(String, String)]) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[])
                .with_context(context)
                .with_published_keys(PUBLISHED_CONDITION_KEYS),
        )
    }

    /// RULE: the condition keys this gate carries are the two this service publishes and the
    /// global tag keys, so a grant scoped by any of them is decided here as IAM decides it.
    #[test]
    fn a_grant_scoped_by_a_published_condition_key_is_decided() {
        let request = serde_json::json!({
            "ServiceNamespace": "dynamodb",
            "ResourceId": "table/orders",
            "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
            "Tags": {"team": "beta"},
        });
        let addressed = Addressed {
            resource: "arn:aws:application-autoscaling:us-east-1:000000000000:scalable-target/one"
                .to_string(),
            resource_tags: vec![("owner".to_string(), "search".to_string())],
        };
        let context = condition_context(&request, &addressed);
        // (case, the clause a grant is scoped by, whether it holds for that request)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "the dimension the request names",
                r#"{"StringEquals":{"application-autoscaling:scalable-dimension":"dynamodb:table:ReadCapacityUnits"}}"#,
                Decision::Allowed,
            ),
            (
                "another dimension",
                r#"{"StringEquals":{"application-autoscaling:scalable-dimension":"dynamodb:table:WriteCapacityUnits"}}"#,
                Decision::NotStated,
            ),
            (
                "the namespace the request names",
                r#"{"StringEquals":{"application-autoscaling:service-namespace":"dynamodb"}}"#,
                Decision::Allowed,
            ),
            (
                "another namespace",
                r#"{"StringEquals":{"application-autoscaling:service-namespace":"ecs"}}"#,
                Decision::NotStated,
            ),
            (
                "a tag the request carries",
                r#"{"StringEquals":{"aws:RequestTag/team":"beta"}}"#,
                Decision::Allowed,
            ),
            (
                "another value of that tag",
                r#"{"StringEquals":{"aws:RequestTag/team":"alpha"}}"#,
                Decision::NotStated,
            ),
            (
                "a tag the addressed target holds",
                r#"{"StringEquals":{"aws:ResourceTag/owner":"search"}}"#,
                Decision::Allowed,
            ),
            (
                "another value of that resource tag",
                r#"{"StringEquals":{"aws:ResourceTag/owner":"orders"}}"#,
                Decision::NotStated,
            ),
            (
                "the tag keys the request names",
                r#"{"ForAllValues:StringEquals":{"aws:TagKeys":["team"]}}"#,
                Decision::Allowed,
            ),
        ];
        for (case, clause, expected) in rows {
            let document = format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"{ACTION_PREFIX}:RegisterScalableTarget","Resource":"*","Condition":{clause}}}]}}"#
            );
            let reached = decision(
                &document,
                &format!("{ACTION_PREFIX}:RegisterScalableTarget"),
                &addressed.resource,
                &context,
            );
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    /// RULE: a registration carrying tags needs the tagging permission as well, so a policy
    /// that grants only the registration does not cover it.
    #[test]
    fn a_tag_carrying_request_also_needs_the_tagging_permission() {
        // (case, the request document, whether the tagging action is also needed)
        let rows: &[(&str, Value, bool)] = &[
            (
                "a registration carrying tags",
                serde_json::json!({"Tags": {"team": "beta"}}),
                true,
            ),
            (
                "a registration carrying an empty tag map",
                serde_json::json!({"Tags": {}}),
                false,
            ),
            (
                "a registration carrying no tags at all",
                serde_json::json!({"ResourceId": "table/orders"}),
                false,
            ),
        ];
        for (case, request, expected) in rows {
            let actual = carries_tags(request);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the tagging permission to be needed == {expected}, \
                 got {actual}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules refused — every part taken from the request and the stored
    /// policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let caller = Caller {
            arn: "arn:aws:sts::000000000000:assumed-role/aas-reader/session-1".to_string(),
            principals: Vec::new(),
            documents: Vec::new(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
        };
        let action = format!("{ACTION_PREFIX}:DeregisterScalableTarget");
        let resource = every_scalable_target("us-east-1");
        let reason = format!("because no identity-based policy allows the {action} action");
        let message = caller.refused(&action, &resource, &reason).message();
        assert_eq!(
            message,
            format!(
                "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
                caller.arn
            ),
            "case \"an unauthorized deregistration\": expected IAM's wording, got {message:?}"
        );
    }
}
