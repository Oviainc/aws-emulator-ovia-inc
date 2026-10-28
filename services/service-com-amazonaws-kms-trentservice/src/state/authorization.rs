// SPDX-License-Identifier: CC0-1.0

//! Who a request to this service speaks for, and whether the authorities in force let it
//! do what it asks.
//!
//! KMS is one of the services whose permission model is not "the caller is the account":
//! a request's permission comes from one of THREE authorities, and a request none of them
//! allows is refused with `AccessDeniedException` (HTTP 400) before anything is read or
//! written:
//!
//! 1. the KEY's own key policy, the resource-based document the key carries
//!    ([`super::Cmk::policy`], which `CreateKey`/`PutKeyPolicy` wrote);
//! 2. the calling principal's IDENTITY-based policies, read out of the IAM store;
//! 3. the GRANTS on that key the caller holds — the ones whose grantee principal is the
//!    caller, and the ones the request presents a grant token for. "Grants are an
//!    alternative to key policies" and they are withdrawn by `RetireGrant` /
//!    `RevokeGrant`, so the authority disappears from the very next request.
//!
//! One divergence from the real service is deliberate and is the one the handlers'
//! module docs record: a request that PRESENTS a grant token is treated as drawing its
//! permission from the grants those tokens name, rather than additively beside the two
//! policies. See [`authorize`].
//!
//! An operation that names NO key has no key policy to ask, so only the second authority
//! is in force: its permission comes from the calling principal's identity-based policies
//! alone, over the whole service in this Region ([`authorize_account_action`]).
//!
//! The pieces, each held in exactly one place — the same shape every other service's own
//! `state/authorization.rs` takes, because this is one rule and not several:
//!
//! * **who** — the principal the request's access key id speaks for: the session STS
//!   minted that key for (the same place `sts:GetCallerIdentity` answers from), else the
//!   IAM user that owns the long-term key, else the account's own published credential.
//!   Resolving it materialises the session expiries the clock has already made due, a
//!   write owed at that instant whether or not anybody called.
//! * **the documents** — the key policy out of this service's own store, and the
//!   principal's inline and attached policies out of IAM's own store, so this service
//!   sees exactly what those handlers wrote.
//! * **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!   evaluation of policy documents against `(action, resource, principal)`, so this
//!   service and every other one handed the same document decide alike.
//!
//! ## Why a statement naming the ACCOUNT is not a grant on its own
//!
//! The default key policy "gives the Amazon Web Services account that owns the KMS key
//! permission to use it in all cryptographic operations"
//! (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`) by naming the
//! account's root user. Such a statement DELEGATES the decision to that account's own
//! identity-based policies rather than granting to every principal in the account: a
//! principal whose identity policies say nothing is not allowed by it, while the
//! account's own credential — which no identity policy governs — is. A statement naming
//! the principal ITSELF grants outright. The two principal spellings that express that
//! split are the shared evaluator's ([`iam::account_principals`]); a DENY is decided
//! against both sets at once, because denying an account denies each of its principals.
//!
//! ## The callers that are not IAM principals
//!
//! * **The account's own published credential.** A request whose access key id neither
//!   STS nor IAM holds a record for speaks for the account itself, which holds no
//!   identity policies — the position AWS account root credentials are in.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-MANAGED
//!   policy's document belongs to AWS, not to this account, so refusing on evidence this
//!   emulator does not have would deny access AWS permits.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused.
//!
//! ## The condition keys a statement is judged under
//!
//! A statement applies to a request only when the `Condition` block it carries holds for
//! that request, whichever effect it states — so a statement scoped to one principal
//! governs that principal alone. The keys this gate can answer EXACTLY are the ones that
//! name the signing principal, and they are in the request context of every call: the ARN
//! of the principal that made the request and the account it belongs to
//! ([`condition_context`]). Every other key — including this service's own `kms:` keys — is
//! left unpublished, so a statement naming one stays undecided and the shared evaluator
//! reads it fail-closed.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The global condition keys that name the PRINCIPAL a request was signed by, which this
/// gate answers exactly: "aws:PrincipalArn — the ARN of the principal that made the
/// request" and "aws:PrincipalAccount — the account the principal belongs to" (IAM's
/// global condition keys).
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// The principal a request speaks for, with everything a decision about it is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The spellings a policy statement names this principal ITSELF by: the session ARN
    /// STS reports, and the ARN of the IAM role behind an assumed-role session. A
    /// statement naming one of these grants to the caller outright.
    pub principals: Vec<String>,
    /// The spellings a statement names the caller's whole ACCOUNT by — see the module
    /// docs for what naming the account does and does not grant.
    pub account: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when this request presents the account's own published credential.
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does
    /// not hold — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether `grant` was issued to THIS caller, so the permissions it carries are the
    /// caller's: "the grantee principal" is the identity a grant gives permission to, and
    /// a session of a role is the role it is a session of.
    #[must_use]
    pub fn holds(&self, grant: &super::Grant) -> bool {
        grant.grantee_principal.as_deref().is_some_and(|grantee| {
            self.principals
                .iter()
                .any(|principal| principal == grantee)
        })
    }

    /// Whether this caller's identity-based half of the decision cannot refuse: the
    /// account's own credential, or a principal carrying a grant this emulator cannot
    /// read (see the module docs).
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// Whether the authorities in force let `caller` perform `kms:<operation>` on `key`, and
/// the refusal message when they do not.
///
/// `presented` is `Some` of the grants the request's own grant tokens name — the handler
/// resolves them, because a token this service never minted is a different, modelled
/// refusal — and `None` for a request that presents no grant token at all. A request that
/// DOES present one is treated as drawing its permission from the grants those tokens
/// name, which is narrower than the real service, where a grant token is additive: that
/// is this emulator's one documented divergence here, and each handler's module doc
/// records it.
///
/// `context` is the request's encryption context, which a grant's
/// `EncryptionContextEquals` / `EncryptionContextSubset` constraint is judged against and
/// which an operation that takes none passes as `None`.
pub fn authorize(
    caller: &Caller,
    state: &super::ComAmazonawsKmsTrentserviceState,
    region: &str,
    key: &super::Cmk,
    operation: &str,
    context: Option<&HashMap<String, String>>,
    presented: Option<&[&super::Grant]>,
) -> Result<(), String> {
    let action = format!("{}:{operation}", super::ARN_SERVICE);

    // The condition keys this request answers for, and — since every key it carries is one
    // this gate knows the request's value for — the keys the request DECLARES it models.
    // Without them a statement scoped by a condition could not be decided at all, and the
    // evaluator's fail-closed reading would apply a `Deny` to principals it does not name.
    let principal_context = condition_context(caller);
    let published: Vec<&str> = principal_context
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    // An explicit `Deny` wins over every grant, whichever document states it, and
    // denying the ACCOUNT denies each of its principals — so the deny is decided against
    // the caller's own spellings and its account's together.
    let mut denying = caller.principals.clone();
    denying.extend_from_slice(&caller.account);
    let deny_request = iam::Request::new(&action, &key.arn, &denying)
        .with_context(&principal_context)
        .with_published_keys(&published);
    if iam::resource_decision(&key.policy, &deny_request) == Decision::Denied {
        return Err(explicit_deny_message(
            &caller.arn,
            &action,
            &key.arn,
            "a resource-based policy",
        ));
    }
    let granting = iam::Request::new(&action, &key.arn, &caller.principals)
        .with_context(&principal_context)
        .with_published_keys(&published);
    let identity_said = iam::identity_decision(&caller.documents, &granting);
    if identity_said == Decision::Denied {
        return Err(explicit_deny_message(
            &caller.arn,
            &action,
            &key.arn,
            "an identity-based policy",
        ));
    }

    // A grant permits exactly the operations it lists, on the one key it covers, under
    // its own encryption-context constraints. The grants in force are those the request's
    // tokens name — and, for a request that presents none, those ISSUED to the caller:
    // "the grantee principal" holds the grant's permissions without presenting anything,
    // which is why `RetireGrant` and `RevokeGrant` withdraw its access.
    if let Some(presented) = presented {
        return presented
            .iter()
            .any(|grant| grant.permits(&key.key_id, operation, context))
            .then_some(())
            .ok_or_else(|| no_identity_policy_message(&caller.arn, &action, &key.arn));
    }
    let by_grant = state
        .grants_for_key(region, &key.key_id)
        .into_iter()
        .any(|grant| caller.holds(grant) && grant.permits(&key.key_id, operation, context));
    if by_grant {
        return Ok(());
    }

    // A key-policy statement naming the principal itself grants outright; one naming the
    // account delegates to that account's identity policies (see the module docs).
    if iam::resource_decision(&key.policy, &granting) == Decision::Allowed {
        return Ok(());
    }
    let delegates = !caller.account.is_empty()
        && iam::resource_decision(
            &key.policy,
            &iam::Request::new(&action, &key.arn, &caller.account)
                .with_context(&principal_context)
                .with_published_keys(&published),
        ) == Decision::Allowed;
    if delegates && (caller.identity_is_unbounded() || identity_said == Decision::Allowed) {
        return Ok(());
    }

    // WHICH document has to change is what AWS's two implicit-deny sentences say, and the
    // key policy decides which one applies: a policy that does not allow the action for
    // this caller or its account withholds it outright, and no identity policy can widen a
    // key policy that grants nothing — while a policy that DELEGATES to the account leaves
    // the principal's own identity policies as the permission it is missing.
    Err(if delegates {
        no_identity_policy_message(&caller.arn, &action, &key.arn)
    } else {
        no_resource_policy_message(&caller.arn, &action, &key.arn)
    })
}

/// Whether the authorities in force let `caller` perform an ACCOUNT-LEVEL
/// `kms:<operation>` — one that names no KMS key — and the refusal message when they do
/// not.
///
/// Such an operation reaches no key, so no key policy is in force and the whole decision
/// is the calling principal's identity-based policies: the permission KMS documents for
/// one of these is an IAM-policy permission
/// (`smithy_query(query="doc",shape="ListAliases")`: "Required permissions:
/// kms:ListAliases (IAM policy)"), and a principal whose identity policies do not allow
/// the action holds no other authority to fall back on — a grant covers one key and a key
/// policy governs one key, so neither can carry a request that is about neither.
///
/// The resource the request is judged against is the whole service in `region`
/// ([`account_resource`]), because an action with no resource type is written in a policy
/// against the service's wildcard ARN.
pub fn authorize_account_action(
    caller: &Caller,
    region: &str,
    operation: &str,
) -> Result<(), String> {
    let action = format!("{}:{operation}", super::ARN_SERVICE);
    let resource = account_resource(region);

    // The same request context every key-scoped decision publishes, so one statement
    // scoped to a principal is read the same way whichever operation asks.
    let principal_context = condition_context(caller);
    let published: Vec<&str> = principal_context
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let request = iam::Request::new(&action, &resource, &caller.principals)
        .with_context(&principal_context)
        .with_published_keys(&published);

    match iam::identity_decision(&caller.documents, &request) {
        // An explicit `Deny` wins over every `Allow`, whichever of the caller's documents
        // states it.
        Decision::Denied => Err(explicit_deny_message(
            &caller.arn,
            &action,
            &resource,
            "an identity-based policy",
        )),
        Decision::Allowed => Ok(()),
        // A caller whose identity-based half cannot refuse (see the module docs) is
        // served: refusing on evidence this emulator does not hold would deny access AWS
        // permits.
        _ if caller.identity_is_unbounded() => Ok(()),
        _ => Err(no_identity_policy_message(&caller.arn, &action, &resource)),
    }
}

/// The resource an ACCOUNT-LEVEL action is judged against: every KMS resource of this
/// account in `region`, which is what an action carrying no resource type is written
/// against in a policy. Built through [`super::ARN_SERVICE`] and the engine's one ARN
/// builder, the same way [`super::key_arn`] names a key.
#[must_use]
pub fn account_resource(region: &str) -> String {
    ::engine::emulator_arn(super::ARN_SERVICE, region, "*")
}

/// The whole account-level gate an implemented operation needs, on that operation's own
/// error channel: who the request speaks for, whether its identity-based policies allow
/// `kms:<operation>` in the Region the request was signed for, and the
/// `AccessDeniedException` (HTTP 400) that refuses it when they do not.
///
/// One call per handler, so no two account-level operations gate themselves differently —
/// and it reaches no store of this service, so a handler calls it before locking its own.
pub fn check_account_action<E, Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let caller = caller(engine, ctx);
    let region = super::request_region(ctx);
    authorize_account_action(&caller, &region, operation)
        .map_err(crate::scaffolded::Scaffolded::access_denied)
}

/// The condition keys a statement of these documents is judged under: the keys that name
/// the signing principal, which every request carries a value for.
///
/// A key that is in this context is one the request DECLARES it models, so a clause naming
/// it is DECIDED — which is what scopes a conditioned statement to the principals it names.
/// A caller that speaks for nobody (an expired session credential) carries no principal
/// spelling, so there is nothing to compare and the context is empty; the account is read
/// out of the principal's own ARN through the tree's one ARN parse, so an ARN carrying none
/// publishes no account key rather than a guess.
fn condition_context(caller: &Caller) -> Vec<(String, String)> {
    if caller.principals.is_empty() {
        return Vec::new();
    }
    let mut context = vec![(PRINCIPAL_ARN_KEY.to_string(), caller.arn.clone())];
    let account = ::service_common::arn::parse(&caller.arn)
        .map(|arn| arn.account)
        .filter(|account| !account.is_empty());
    if let Some(account) = account {
        context.push((PRINCIPAL_ACCOUNT_KEY.to_string(), account.to_string()));
    }
    context
}

/// How AWS words an authorization failure that no document allowed: who was refused,
/// which action, on which resource, and that no identity-based policy allows the action.
/// Every value in it comes from the request and the caller's credential.
///
/// One function for the whole service, so no two operations word the same refusal
/// differently.
#[must_use]
pub fn no_identity_policy_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// How AWS words an authorization failure the RESOURCE's own policy withheld: who was
/// refused, which action, on which resource, and that no resource-based policy allows the
/// action — the sentence for a key whose policy does not allow the action for this caller
/// or for its account at all, which is the document that has to change.
///
/// One function for the whole service, so no two operations word the same refusal
/// differently.
#[must_use]
pub fn no_resource_policy_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no resource-based policy allows the {action} action"
    )
}

/// How AWS words an authorization failure a document explicitly denied, naming which kind
/// of document denied it.
#[must_use]
fn explicit_deny_message(principal: &str, action: &str, resource: &str, document: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in {document}"
    )
}

/// The principal this request speaks for, and the documents in force on it.
///
/// Called BEFORE this service's own store is locked, so the STS and IAM stores it reads
/// are never reached while this service's state guard is held.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
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
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                principals: Vec::new(),
                account: Vec::new(),
                documents: Vec::new(),
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        account: iam::account_principals(&identity.arn),
        arn: identity.arn,
        documents: Vec::new(),
        unreadable_attachment: false,
    };
    let Some(iam_state) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam_state.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it
    // never minted is the account's ambient identity — but IAM's store maps an access key
    // id to the user that owns it, so a request signed with one speaks for THAT user and
    // is judged on the user's policies rather than served as the account.
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
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        if let Some(held) = store.roles.get(&role_key) {
            caller.principals.push(held.arn.clone());
        }
        caller
            .documents
            .extend(inline_documents(&store.role_inline_policies, &role_key));
        for attached in store
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                // Only an AWS-MANAGED policy is a grant this emulator cannot read.
                None if is_aws_managed_policy(attached) => {
                    caller.unreadable_attachment = true;
                }
                None => {}
            }
        }
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        // A user's INLINE policies are what the IAM store holds for it; there is no
        // attached-policy map to read for a user.
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
    caller
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's `aws`,
/// so it belongs to AWS rather than to this account and the IAM store holds no document
/// for it. Read through IAM's own ARN parser, so this service and IAM agree on what an
/// AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_store::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_store::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role
/// is the first segment after `assumed-role/`.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The resource part of an ARN whose service segment is `service`.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let parsed = ::service_common::arn::parse(arn)?;
    (parsed.vendor == service).then_some(parsed.resource)
}

/// The inline policy documents one entity holds, from the IAM store's
/// `<entity> -> <policy name> -> policy` map.
fn inline_documents(
    policies: &BTreeMap<String, BTreeMap<String, iam_store::InlinePolicy>>,
    entity_key: &str,
) -> Vec<String> {
    policies
        .get(entity_key)
        .map(|held| {
            held.values()
                .map(|policy| policy.policy_document.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The document of a managed policy's DEFAULT version — the one in force on every entity
/// it is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_store::ManagedPolicy) -> Option<String> {
    let default = managed.policy.default_version_id.as_deref();
    managed
        .versions
        .values()
        .find(|version| default.is_some() && version.version_id.as_deref() == default)
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}
