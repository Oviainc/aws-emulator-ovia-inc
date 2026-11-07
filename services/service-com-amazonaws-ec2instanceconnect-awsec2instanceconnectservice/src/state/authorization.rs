// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal a request speaks for push the key it asks to push.
//!
//! Both operations of this service are signed calls (`sigv4`,
//! `smithy_query(query="service_info")`, with no `@optionalAuth` operation in the model), and
//! AWS decides such a call against the CALLER's identity-based policies BEFORE the operation
//! resolves anything: a caller holding no grant is told it is not authorized rather than told
//! whether the instance it named exists, and a refused push stores no key. So the decision is
//! taken in front of the instance lookup, and only a caller the policies in force allow
//! reaches it.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a session
//!    credential, and from IAM's own `access_keys` map (the map `iam:GetUser` resolves a
//!    caller through) for a long-term key, so the principal a refusal blames is the one the
//!    caller reads back for the credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents in force on that principal and
//!    the permissions boundary that caps them, read through the IAM service's OWN accessors
//!    (`principal_policies`, `permissions_boundary_document`), where the collection rule
//!    itself lives: the principal's inline policies, the default version of each managed
//!    policy attached to it and, for a user, the policies in force on the groups it belongs
//!    to — so this service sees exactly what IAM's own authorizer sees, and a policy just put
//!    grants immediately while one just deleted stops granting immediately;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation of
//!    those documents against `(action, resource)` and the request's condition context, so
//!    this service and every other one handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does — the same
//! split every other service's gate makes, and the lookups stay in this crate because
//! hoisting them into `service-common` would make every service depending on that crate link
//! the IAM and STS service crates.
//!
//! ## What the decision is made ABOUT
//!
//! The action is this service's own, `ec2-instance-connect:<OperationName>`
//! ([`action_for`], over the `arnNamespace` the model declares), and the resource is the EC2
//! INSTANCE the request names ([`instance_resource`]) — an instance is EC2's resource, so the
//! ARN a statement has to match is the one EC2 mints for it. A policy may further narrow the
//! grant with a `Condition`, and the keys this service can answer for are published to the
//! evaluation ([`Conditions`]): the OS user a key is pushed for, and the tags of the instance
//! the push names, under both spellings AWS publishes a resource tag as.
//!
//! ## The refusal
//!
//! A call no policy allows, or one an explicit `Deny` refuses, is answered with the error code
//! `AccessDeniedException` and HTTP 403 — the answer AWS's authorization layer gives, which is
//! not one of the errors this service's own model declares, so it is carried by
//! [`crate::scaffolded::Scaffolded::access_denied`] rather than by a modelled shape. The
//! message is built from the three things the decision was made about — the principal, the
//! action and the resource — closed by which document withheld the grant
//! ([`not_authorized`]), and the withheld push leaves a structured audit record ([`refusal`]).
//!
//! ## The two callers the decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS nor
//!   IAM holds a record for resolves to [`sts_store::ambient_identity`], the account's own
//!   identity, which is not a principal the IAM store holds policies for — the same position
//!   AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy is
//!   owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]). A principal with such a policy attached is
//!   allowed: this service cannot see what that document grants, and refusing on evidence it
//!   does not have would deny access AWS permits. An attachment naming a CUSTOMER-managed
//!   policy the account does not hold grants nothing, because the account is the authority on
//!   its own policies.
//!
//! An expired session credential speaks for nobody: it carries no document and no principal
//! spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which materialises the
//! session expiries the clock has already made due before it answers: that write is owed at
//! the same instant whether or not anybody called.
//!
//! ## What one decision costs, and what it reads
//!
//! The work is bounded by what IAM holds for ONE principal and does not grow with the
//! request: the inline policies embedded in that principal, the managed policies attached to
//! it and the groups it belongs to, each document collected once by IAM's own accessor and
//! read once by the shared evaluator, whose matcher is single-pass and never recurses. The
//! decision is taken with no store guard of this service held, and it writes nothing to either
//! store it reads. Every value it reads comes from the request (the access key id in the
//! credential scope, the Region it was signed for, the members it carries) and from those two
//! in-process stores: nothing here reads the host's environment or filesystem, opens a socket
//! or spawns anything, every lookup answers an `Option`, and no refusal quotes a value back
//! except the principal's own ARN, the action and the instance ARN the request addressed.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `ec2-instance-connect:<OperationName>`
/// ("Actions defined by Amazon EC2 Instance Connect", IAM Service Authorization Reference),
/// which is also this service's `arnNamespace`
/// (`smithy_query(query="service_info")`: `arnNamespace: ec2-instance-connect`).
pub const ACTION_PREFIX: &str = "ec2-instance-connect";

/// The service segment of the ARN a key push is decided about.
///
/// The target of both operations is an EC2 INSTANCE, which is Amazon EC2's resource
/// (`arn:aws:ec2:<region>:<account>:instance/<instance id>`), so a statement granting either
/// action names the instance under EC2's namespace rather than under this service's.
const INSTANCE_ARN_NAMESPACE: &str = "ec2";

/// The ARN resource type an instance is spelled under, as EC2 mints it.
const INSTANCE_RESOURCE_TYPE: &str = "instance";

/// The condition key carrying the OS user a key is pushed for: `ec2:osuser`, which AWS
/// documents for `ec2-instance-connect:SendSSHPublicKey` ("the OS user name for the instance
/// you are connecting to"). A serial-console push names no OS user, so it publishes no value
/// for it and no clause over it can be decided — see [`Conditions::new`].
pub const OS_USER_KEY: &str = "ec2:osuser";

/// The GLOBAL spelling of the condition key of ONE tag of the resource the request addresses:
/// "`aws:ResourceTag/tag-key` — compares the tag key-value pair that you specify in the policy
/// with the key-value pair attached to the resource".
const AWS_RESOURCE_TAG_PREFIX: &str = "aws:ResourceTag/";

/// The same key in Amazon EC2's own namespace (`ec2:ResourceTag/tag-key`), the spelling an
/// identity policy about an EC2 resource uses. Both name the tags of the instance the push is
/// taken on, so both are answered from the same tag set.
const EC2_RESOURCE_TAG_PREFIX: &str = "ec2:ResourceTag/";

/// The IAM action `operation` is performed under: `ec2-instance-connect:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The ARN of the EC2 instance a push names, as EC2 mints it for a request signed for
/// `region` — the resource an authorization decision is made about.
///
/// Built from the id the REQUEST carried, before anything is looked up, so the decision does
/// not depend on whether that instance exists: a caller holding no grant must not learn from
/// the refusal what the account holds.
#[must_use]
pub fn instance_resource(region: &str, instance_id: &str) -> String {
    ::emulator_server::emulator_arn(
        INSTANCE_ARN_NAMESPACE,
        region,
        &format!("{INSTANCE_RESOURCE_TYPE}/{instance_id}"),
    )
}

/// The condition keys one push can be decided over: the values it carries, and the keys it
/// MODELS whether or not it carries a value for each.
///
/// A key declared here and absent from the values is one the request carries no value for,
/// which is what lets a clause over it be decided at all; every OTHER key stays undeclared,
/// and the shared evaluation leaves a clause over one of those UNDECIDED — an `Allow` that may
/// not hold grants nothing and a `Deny` that may hold still refuses, which is fail-closed in
/// both directions.
#[derive(Debug, Clone, Default)]
pub struct Conditions {
    /// The `(key, value)` pairs this push knows.
    context: Vec<(String, String)>,
    /// The keys it models, families included (a trailing `/` names the whole family).
    published: Vec<&'static str>,
}

impl Conditions {
    /// The condition context of one push: the OS user it names, where the operation takes one,
    /// and the tags of the instance it addresses, where the tag set is KNOWN in full.
    ///
    /// The tag families are published only for a `tags` the owner of the instance answered
    /// with: a key of the family that is then absent is a tag the instance does not carry,
    /// which is a fact. For an instance nothing could answer for, the tag set is not known —
    /// not known to be empty — so neither family is published and a clause over one of them
    /// leaves its statement undecided.
    #[must_use]
    pub fn new(os_user: Option<&str>, tags: Option<&BTreeMap<String, String>>) -> Self {
        let mut conditions = Self::default();
        if let Some(os_user) = os_user {
            conditions
                .context
                .push((OS_USER_KEY.to_string(), os_user.to_string()));
            conditions.published.push(OS_USER_KEY);
        }
        if let Some(tags) = tags {
            for (key, value) in tags {
                for prefix in [AWS_RESOURCE_TAG_PREFIX, EC2_RESOURCE_TAG_PREFIX] {
                    conditions
                        .context
                        .push((format!("{prefix}{key}"), value.clone()));
                }
            }
            conditions
                .published
                .extend_from_slice(&[AWS_RESOURCE_TAG_PREFIX, EC2_RESOURCE_TAG_PREFIX]);
        }
        conditions
    }
}

/// The refusal AWS words an authorization failure with: who was refused, which action, on
/// which resource, and which rule withheld the grant. Every value in it comes from the request
/// and the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// `Some(message)` when IAM does not allow `ec2-instance-connect:<operation>` on `resource`
/// for the calling principal — the message the refusal carries — and `None` when it does.
///
/// A refusal leaves a structured audit record: the operation asked for, the access key id the
/// request presented, the principal it resolved to, the action and the resource decided about,
/// and the outcome — so a push that was withheld is recorded locally. Only wire metadata and
/// stored identifiers; no credential material and no part of the key material the request
/// carried.
///
/// Asked with NO store guard held, and before the operation reads or writes anything of its
/// own.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    conditions: &Conditions,
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    let withheld = withheld_grant(&caller, &action, resource, conditions);
    if withheld.is_some() {
        ::tracing::info!(
            audit = true,
            operation = %operation,
            outcome = "refused_not_authorized",
            caller_akid = %::emulator_server::extract_akid(&ctx.parts().headers),
            principal = %caller.arn,
            action = %action,
            resource = %resource,
            "no key was pushed"
        );
    }
    withheld
}

/// The decision itself, in IAM's own order: an explicit `Deny` in a document that CAN be read
/// refuses the request whatever else the principal carries; otherwise a grant is needed, which
/// a caller this gate does not bound has by definition (see the module docs); and a grant is
/// capped by the permissions boundary, since "a permissions boundary sets the maximum
/// permissions that an identity-based policy can grant" (IAM, permissions boundaries).
fn withheld_grant(
    caller: &Caller,
    action: &str,
    resource: &str,
    conditions: &Conditions,
) -> Option<String> {
    // An identity-based document states no `Principal` element, so the decision rests on the
    // action, the resource and the request's condition context.
    let request = iam::Request::new(action, resource, &[])
        .with_context(&conditions.context)
        .with_published_keys(&conditions.published);
    let said = iam::identity_decision(&caller.documents, &request);
    if said == Decision::Denied {
        return Some(not_authorized(
            &caller.arn,
            action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(not_authorized(
            &caller.arn,
            action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
        Decision::NotStated => {
            format!("because no permissions boundary allows the {action} action")
        }
    };
    Some(not_authorized(&caller.arn, action, resource, &reason))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
    pub boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
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
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
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
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
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
    // user that holds it (`access_keys`, the map `iam:GetUser` resolves a caller through), so
    // a request signed with one speaks for THAT user and is judged on the user's policies
    // rather than served as the account. A key the IAM store does not hold either really is
    // the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults, with the condition context `conditions` publishes.
    fn decision(
        document: &str,
        action: &str,
        resource: &str,
        conditions: &Conditions,
    ) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[])
                .with_context(&conditions.context)
                .with_published_keys(&conditions.published),
        )
    }

    /// The action is this service's own prefix plus the operation, for every operation the
    /// model binds.
    #[test]
    fn the_action_is_this_services_prefix_and_the_operation() {
        for operation in ["SendSSHPublicKey", "SendSerialConsoleSSHPublicKey"] {
            let action = action_for(operation);
            assert_eq!(
                action,
                format!("ec2-instance-connect:{operation}"),
                "case {operation:?}: expected the service's own action, got {action:?}"
            );
        }
    }

    /// The resource is the EC2 instance ARN of the id the request carried, in the Region it
    /// was signed for.
    #[test]
    fn the_resource_is_the_instances_own_arn() {
        for (region, instance_id) in [
            ("us-east-1", "i-0a1b2c3d4e5f60718"),
            ("eu-west-2", "i-00000000000000001"),
        ] {
            let arn = instance_resource(region, instance_id);
            assert_eq!(
                arn,
                format!(
                    "arn:aws:ec2:{region}:{}:instance/{instance_id}",
                    ::engine::EMULATOR_ACCOUNT_ID
                ),
                "case {region}/{instance_id}: expected EC2's own instance ARN, got {arn:?}"
            );
        }
    }

    /// A grant is read for the action and the resource it names: a statement about another
    /// action, or about another instance, states nothing about this push.
    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let resource = instance_resource("us-east-1", "i-0a1b2c3d4e5f60718");
        let conditions = Conditions::new(Some("ec2-user"), Some(&BTreeMap::new()));
        let rows = [
            ("ec2-instance-connect:SendSSHPublicKey", "*", Decision::Allowed),
            (
                "ec2-instance-connect:*",
                resource.as_str(),
                Decision::Allowed,
            ),
            ("ec2-instance-connect:SendSerialConsoleSSHPublicKey", "*", Decision::NotStated),
            (
                "ec2-instance-connect:SendSSHPublicKey",
                "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000009",
                Decision::NotStated,
            ),
        ];
        for (granted_action, granted_resource, expected) in rows {
            let document = format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                   "Action":"{granted_action}","Resource":"{granted_resource}"}}]}}"#
            );
            let said = decision(
                &document,
                &action_for("SendSSHPublicKey"),
                &resource,
                &conditions,
            );
            assert_eq!(
                said, expected,
                "case {granted_action} on {granted_resource}: expected {expected:?}, got \
                 {said:?}"
            );
        }
    }

    /// A clause over a key this push publishes is DECIDED — the OS user it named and the tags
    /// of the instance it addressed — while the same clause on an instance whose tag set is
    /// not known leaves the statement undecided, so the grant is withheld.
    #[test]
    fn a_clause_over_a_published_key_is_decided_from_the_request() {
        let resource = instance_resource("us-east-1", "i-0a1b2c3d4e5f60718");
        let tagged: BTreeMap<String, String> =
            [("Environment".to_string(), "staging".to_string())]
                .into_iter()
                .collect();
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
               "Action":"{}","Resource":"*","Condition":{{"StringEquals":{{
               "aws:ResourceTag/Environment":"staging","ec2:osuser":"ec2-user"}}}}}}]}}"#,
            action_for("SendSSHPublicKey")
        );
        let rows = [
            ("the tag and the OS user match", Some("ec2-user"), Some(&tagged), Decision::Allowed),
            ("another OS user", Some("root"), Some(&tagged), Decision::NotStated),
            ("no tag of that key", Some("ec2-user"), Some(&BTreeMap::new()), Decision::NotStated),
            ("a tag set nothing answered for", Some("ec2-user"), None, Decision::NotStated),
        ];
        for (case, os_user, tags, expected) in rows {
            let said = decision(
                &document,
                &action_for("SendSSHPublicKey"),
                &resource,
                &Conditions::new(os_user, tags),
            );
            assert_eq!(
                said, expected,
                "case {case:?}: expected {expected:?}, got {said:?}"
            );
        }
    }

    /// An explicit `Deny` about the request wins over every `Allow`, whichever order they are
    /// written in.
    #[test]
    fn an_explicit_deny_wins_over_an_allow() {
        let resource = instance_resource("us-east-1", "i-0a1b2c3d4e5f60718");
        let action = action_for("SendSSHPublicKey");
        let allow = format!(
            r#"{{"Effect":"Allow","Action":"{action}","Resource":"*"}}"#
        );
        let deny = format!(r#"{{"Effect":"Deny","Action":"{action}","Resource":"*"}}"#);
        for (case, statements) in [
            ("the deny second", format!("{allow},{deny}")),
            ("the deny first", format!("{deny},{allow}")),
        ] {
            let document =
                format!(r#"{{"Version":"2012-10-17","Statement":[{statements}]}}"#);
            let said = decision(
                &document,
                &action,
                &resource,
                &Conditions::new(Some("ec2-user"), Some(&BTreeMap::new())),
            );
            assert_eq!(
                said,
                Decision::Denied,
                "case {case:?}: expected the explicit deny to win, got {said:?}"
            );
        }
    }

    /// The refusal is AWS's sentence: the principal, the action, the resource, and which
    /// document withheld the grant.
    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let action = action_for("SendSSHPublicKey");
        let resource = instance_resource("us-east-1", "i-0a1b2c3d4e5f60718");
        let principal = "arn:aws:iam::000000000000:user/probe";
        for (case, reason) in [
            (
                "no grant",
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "an explicit deny",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
        ] {
            let message = not_authorized(principal, &action, &resource, &reason);
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                ),
                "case {case:?}: expected AWS's refusal sentence, got {message:?}"
            );
        }
    }
}
