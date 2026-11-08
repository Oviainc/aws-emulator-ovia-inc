// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller of an `AWS_IAM`-authorized method invoke it.
//!
//! `Method$authorizationType` gives `AWS_IAM` "for using AWS IAM permissions", and AWS takes
//! TWO decisions for such a request: it AUTHENTICATES the SigV4 credential — which is the
//! emulator's own check, never a service's — and then AUTHORIZES `execute-api:Invoke` on the
//! method the request addressed, for the principal that credential speaks for. Without the
//! second decision an authenticated but unprivileged caller is served, which is the one answer
//! AWS never gives: a federated role session holding no `execute-api:Invoke` grant is refused
//! `403` with the sentence IAM words an authorization failure in.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for ([`sts_store::resolve_caller`], the same record `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for the
//!    credentials it signed with), else the IAM user that owns the long-term key, else the
//!    account's own published credential. A credential an identity pool VENDED is a session of
//!    that registry like any other, which is what makes a Cognito-federated caller a principal
//!    this decision can be taken about at all;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through IAM's own accessors
//!    (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutRolePolicy` wrote takes effect on the next invoke and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared evaluation of
//!    those documents, asked about the method's own ARN
//!    ([`super::super::resource_policy::Call::method_arn`]) — so this gate and the api's
//!    RESOURCE policy, which the data plane applies beside it, are asked about one resource.
//!
//! A refusal states all four: who was refused, the action, the method, and WHY — the clause
//! IAM closes an authorization failure with, naming the document class that refused the call
//! and whether its deny was explicit ([`not_authorized`]). That clause is what tells a caller
//! whether a grant would lift the refusal, so it is part of the answer rather than a comment
//! on it.
//!
//! ## How the two grants combine
//!
//! For a caller in the api's own account either grant suffices: AWS evaluates the api's
//! resource policy and the caller's identity-based policies together, and "if the API Gateway
//! resource policy allows the action, the identity-based policy need not" — so the request is
//! admitted when the document the developer attached allows it OR the caller's own policies do,
//! and refused when neither does. An explicit `Deny` in either refuses whatever the other says:
//! the resource policy's own deny is applied by [`super::super::resource_policy::denial`] ahead
//! of this, and an identity-based deny is applied here.
//!
//! ## The callers this gate does not bound
//!
//! * **the account's own published credential** — a request whose access key id neither STS nor
//!   IAM holds a record for resolves to the account's own identity, which is not a principal
//!   the IAM store holds policies for: the position AWS account root credentials are in, and
//!   the credential every in-process caller of this emulator signs with;
//! * **a principal holding an attachment this emulator cannot read** — an AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it, and
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a boundary the store DOES hold
//!   still caps the grant;
//! * **an engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! Nothing here writes to this service's records. Resolving the caller goes through STS's own
//! `resolve_caller`, which materialises the session expiries the clock has already made due —
//! a write owed at the same instant whether or not anybody invoked anything.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal ARN
//! does. That is the split every other gate in this tree makes (`src/service/authorization.rs`
//! in the acm, bedrock and backup-search crates among them), and the lookups stay in this
//! crate for the reason those keep theirs there: hoisting them into `service-common` would
//! make every service depending on that crate link the IAM and STS service crates. WHICH IAM
//! entity a principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`], and the DECISION through the shared
//! [`::service_common::iam_authorization`].
//!
//! LOCK ORDER: the STS registry's lock and then the IAM store's, each taken and released in
//! turn while this service's own store lock is held. Neither of those services reads this
//! one's state, so that order is the only one taken.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use super::super::resource_policy::{self, INVOKE_ACTION};

/// How an expired session credential's invoke is refused: the sentence AWS words an expired
/// credential with, rather than an authorization-failure sentence naming a principal — that
/// credential speaks for nobody, so there is no principal to name, and the key it presented is
/// not quoted back either. In the assembled emulator the layer that authenticates a request
/// refuses such a credential before this is reached.
const EXPIRED_CREDENTIAL_MESSAGE: &str = "The security token included in the request is expired";

/// The clause IAM closes a refusal with when a readable identity-based document DENIES the
/// action rather than merely not allowing it: adding a grant will not lift such a deny, which
/// is exactly what the clause tells whoever reads the refusal.
const IDENTITY_EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The clause IAM closes a refusal with when the ceiling in force on the principal carries an
/// explicit `Deny` of the action — a fact about the BOUNDARY document, so the refusal names
/// that document class rather than the policies that granted the action.
const BOUNDARY_EXPLICIT_DENY: &str = "with an explicit deny in a permissions boundary";

/// How IAM names the document class that holds what a PRINCIPAL may do, in the clause closing
/// an implicit deny.
const IDENTITY_POLICY_CLASS: &str = "identity-based policy";

/// How IAM names the ceiling in force on a principal, in that same clause.
const BOUNDARY_POLICY_CLASS: &str = "permissions boundary";

/// The clause IAM closes an IMPLICIT deny with: no readable document of `class` allows
/// `action`, so the request fell to IAM's default deny and a grant is what would lift it.
///
/// The action named here is the action the sentence already named, because it is the same
/// decision being explained.
fn no_policy_allows(class: &str, action: &str) -> String {
    format!("because no {class} allows the {action} action")
}

/// The principal an invoke speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct SigningCaller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what the api's resource
    /// policy matches and what a refusal names.
    pub arn: String,
    /// Every spelling of the principal a policy may name it by: the session ARN STS reports,
    /// and the ARN of the IAM role behind an assumed-role session.
    spellings: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on it, when it carries one whose document this
    /// emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has an attachment whose document the IAM store does not hold.
    unreadable_attachment: bool,
    /// Set when the credential the request presented is a session that has already expired, so
    /// it speaks for nobody at all ([`EXPIRED_CREDENTIAL_MESSAGE`]).
    expired: bool,
}

impl SigningCaller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action (see the module docs).
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through IAM's own accessors.
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

/// The principal the credential `access_key_id` speaks for, with the identity-based policies in
/// force on it.
///
/// `credentials` is the emulator's own credential store, which is what STS resolves a session
/// key through; `now` is the instant the session windows are read at.
pub fn signing_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
    credentials: &::emulator_server::CredentialStore,
    now: SystemTime,
) -> SigningCaller {
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(&mut sts.lock(), access_key_id, now, credentials),
        // No STS store on this engine: the only identity a request can present is the account's
        // own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and the invoke it asks for is refused as an expired credential
        // rather than as an authorization failure. The key id it presented is NOT quoted back,
        // and in the assembled emulator the layer that authenticates a request refuses an
        // expired credential before this is reached.
        Err(expired) => {
            let _ = expired;
            return SigningCaller {
                arn: String::new(),
                spellings: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
                expired: true,
            };
        }
    };

    let mut caller = SigningCaller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        spellings: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
        expired: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it, so a request signed with one speaks for THAT user and is judged on the
    // user's policies. A key the IAM store does not hold either really is the account's own
    // published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.spellings = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            let role_arn = held.arn.clone();
            caller.spellings.push(role_arn.clone());
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The sentence an invoke whose credential the clock has already run out on is refused with, or
/// `None` for every live credential.
///
/// Decided BEFORE anything else is asked about the caller — before the api's own resource
/// policy, which [`crate::service::invoke`] applies next — because such a credential speaks for
/// nobody: there is no principal for a statement's `Principal` element to match, so neither a
/// wildcard grant nor a refusal naming a principal is about this request.
#[must_use]
pub fn expired_refusal(caller: &SigningCaller) -> Option<String> {
    caller
        .expired
        .then(|| EXPIRED_CREDENTIAL_MESSAGE.to_string())
}

/// The sentence this invoke is refused with when neither the caller's identity-based policies
/// nor the api's own resource policy allow `execute-api:Invoke` on the method it addressed, or
/// `None` when the request may proceed.
///
/// The sentence closes with the clause IAM words the REASON of the failure in ([`not_authorized`]):
/// which document class refused the call, and whether its deny was explicit or the default one
/// of a call nothing allowed.
///
/// `allowed_by_resource_policy` is whether the document the api carries explicitly ALLOWS this
/// call ([`resource_policy::allows_invoke`]): either grant suffices, so an allow there admits a
/// caller whose own policies say nothing.
#[must_use]
pub fn refusal(
    caller: &SigningCaller,
    call: &resource_policy::Call<'_>,
    allowed_by_resource_policy: bool,
) -> Option<String> {
    if let Some(expired) = expired_refusal(caller) {
        return Some(expired);
    }
    let method_arn = call.method_arn();
    let request = iam::Request::new(INVOKE_ACTION, &method_arn, &caller.spellings);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read refuses
    // the invoke whatever else the principal carries — including a grant this emulator cannot
    // read, which only lifts the DEFAULT deny of a call no readable statement allows.
    if identity_said == Decision::Denied {
        return Some(not_authorized(
            &caller.arn,
            &method_arn,
            IDENTITY_EXPLICIT_DENY,
        ));
    }
    let granted = iam::is_allowed(identity_said, Decision::NotStated)
        || caller.identity_is_unbounded()
        || allowed_by_resource_policy;
    if !granted {
        return Some(not_authorized(
            &caller.arn,
            &method_arn,
            &no_policy_allows(IDENTITY_POLICY_CLASS, INVOKE_ACTION),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant": a grant the boundary does not also allow is not a permission the principal has. A
    // grant the RESOURCE policy made is not an identity-based one, so no boundary caps it.
    if allowed_by_resource_policy || caller.identity_is_unbounded() {
        return None;
    }
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return None,
            // An explicit deny is a fact about the boundary document, so the refusal names it;
            // a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => no_policy_allows(BOUNDARY_POLICY_CLASS, INVOKE_ACTION),
        };
        return Some(not_authorized(&caller.arn, &method_arn, &reason));
    }
    None
}

/// The sentence the `execute-api` endpoint refuses an unauthorized invoke with: the principal
/// that asked, the action it asked for, the method it asked for and `reason` — the clause
/// naming WHICH of the documents consulted refused it and whether that deny was explicit.
///
/// The part up to the resource IS the sentence the api's own resource policy refuses a call
/// with — the one formatter [`resource_policy::not_authorized`] owns, called rather than
/// copied, because it is the same decision about the same method stated by the same endpoint.
/// What this adds is the reason clause, which is a fact about the IDENTITY-based decision
/// taken here: a caller reading it can tell an implicit deny, which a grant would lift, from
/// an explicit one, which no grant lifts.
fn not_authorized(principal: &str, method_arn: &str, reason: &str) -> String {
    format!(
        "{sentence} {reason}",
        sentence = resource_policy::not_authorized(INVOKE_ACTION, principal, method_arn)
    )
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns: how the two grants combine, and the sentence a refusal
    //! states. What a deployed stage SERVES for an `AWS_IAM` method is pinned end to end in
    //! `core/router/tests/all/apigateway_iam_authorized_invoke.rs`.

    use super::*;

    /// One invoke of `GET /orders` on stage `prod` of api `abc1230000`, speaking for `arn`.
    fn call<'a>(arn: &'a str) -> resource_policy::Call<'a> {
        resource_policy::Call {
            region: "us-east-1",
            api_id: "abc1230000",
            stage: "prod",
            verb: "GET",
            resource_path: "/orders",
            request_path: "/orders",
            source_vpce: None,
            caller: resource_policy::Caller::Signed(arn),
        }
    }

    /// A federated role session carrying `documents`, with no boundary and no unreadable
    /// attachment — the position a Cognito-vended credential is in.
    fn session(documents: &[&str]) -> SigningCaller {
        SigningCaller {
            arn: "arn:aws:sts::000000000000:assumed-role/pool-authenticated/CognitoIdentity"
                .to_string(),
            spellings: vec![
                "arn:aws:sts::000000000000:assumed-role/pool-authenticated/CognitoIdentity"
                    .to_string(),
                "arn:aws:iam::000000000000:role/pool-authenticated".to_string(),
            ],
            documents: documents.iter().map(|document| (*document).to_string()).collect(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
            expired: false,
        }
    }

    /// RULE: either grant admits the invoke, neither refuses it, and an explicit deny refuses
    /// it whatever the other grant says.
    #[test]
    fn either_grant_admits_the_invoke_and_neither_refuses_it() {
        let allow_method = "{\"Statement\":[{\"Effect\":\"Allow\",\
                            \"Action\":\"execute-api:Invoke\",\
                            \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                            abc1230000/prod/GET/orders\"}]}";
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let allow_another_method = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                    \"Action\":\"execute-api:Invoke\",\
                                    \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                                    abc1230000/prod/GET/other\"}]}";
        let allow_another_action = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                    \"Action\":\"execute-api:InvalidateCache\",\
                                    \"Resource\":\"*\"}]}";
        let deny_every_invoke = "{\"Statement\":[{\"Effect\":\"Deny\",\
                                 \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        // Rows differ along what the identity-based documents state about this method and along
        // whether the api's own resource policy allows it.
        let rows: &[(&str, &[&str], bool, bool)] = &[
            ("the method itself", &[allow_method], false, true),
            ("every invoke", &[allow_every_invoke], false, true),
            ("another method of the same stage", &[allow_another_method], false, false),
            ("another action on every resource", &[allow_another_action], false, false),
            ("no policy at all", &[], false, false),
            ("no policy, but the resource policy allows", &[], true, true),
            ("an explicit deny", &[deny_every_invoke], false, false),
            (
                "an explicit deny with the resource policy allowing",
                &[deny_every_invoke],
                true,
                false,
            ),
            (
                "an allow beside a deny",
                &[allow_method, deny_every_invoke],
                false,
                false,
            ),
        ];
        for (case, documents, by_resource_policy, admitted) in rows {
            let caller = session(documents);
            let refused = refusal(&caller, &call(&caller.arn.clone()), *by_resource_policy);
            assert_eq!(
                refused.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {refused:?}"
            );
        }
    }

    /// RULE: a permissions boundary caps what an identity-based policy granted — a grant the
    /// boundary does not also allow is not a permission the principal has.
    #[test]
    fn a_permissions_boundary_caps_the_identity_grant() {
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let boundary_other = "{\"Statement\":[{\"Effect\":\"Allow\",\
                              \"Action\":\"execute-api:Invoke\",\
                              \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                              other/prod/GET/orders\"}]}";
        // Rows differ along the boundary in force on the principal.
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("no boundary at all", None, true),
            ("a boundary allowing this method", Some(allow_every_invoke), true),
            ("a boundary allowing only another api", Some(boundary_other), false),
            ("a boundary stating nothing", Some("{\"Statement\":[]}"), false),
        ];
        for (case, boundary, admitted) in rows {
            let mut caller = session(&[allow_every_invoke]);
            caller.boundary = boundary.map(str::to_string);
            let refused = refusal(&caller, &call(&caller.arn.clone()), false);
            assert_eq!(
                refused.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {refused:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the `execute-api:Invoke` action, the ARN of the
    /// method the request addressed and the clause IAM closes an authorization failure with —
    /// which document class refused the call and whether its deny was explicit.
    #[test]
    fn the_refusal_names_the_principal_the_action_the_method_and_the_reason() {
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let deny_every_invoke = "{\"Statement\":[{\"Effect\":\"Deny\",\
                                 \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let boundary_other = "{\"Statement\":[{\"Effect\":\"Allow\",\
                              \"Action\":\"execute-api:Invoke\",\
                              \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                              other/prod/GET/orders\"}]}";
        let sentence = "User: arn:aws:sts::000000000000:assumed-role/pool-authenticated/\
                        CognitoIdentity is not authorized to perform: execute-api:Invoke on \
                        resource: arn:aws:execute-api:us-east-1:000000000000:\
                        abc1230000/prod/GET/orders";
        // Rows differ along WHICH document class refused the invoke and along whether that
        // class stated an explicit deny or merely failed to allow — the four readings this
        // gate can reach.
        let rows: &[(&str, &[&str], Option<&str>, &str)] = &[
            (
                "no document allows the action",
                &[],
                None,
                "because no identity-based policy allows the execute-api:Invoke action",
            ),
            (
                "a readable document denies it",
                &[deny_every_invoke],
                None,
                "with an explicit deny in an identity-based policy",
            ),
            (
                "the boundary states nothing about the action",
                &[allow_every_invoke],
                Some("{\"Statement\":[]}"),
                "because no permissions boundary allows the execute-api:Invoke action",
            ),
            (
                "the boundary allows only another api",
                &[allow_every_invoke],
                Some(boundary_other),
                "because no permissions boundary allows the execute-api:Invoke action",
            ),
            (
                "the boundary denies it",
                &[allow_every_invoke],
                Some(deny_every_invoke),
                "with an explicit deny in a permissions boundary",
            ),
        ];
        for (case, documents, boundary, reason) in rows {
            let mut caller = session(documents);
            caller.boundary = boundary.map(str::to_string);
            let refused = refusal(&caller, &call(&caller.arn.clone()), false)
                .unwrap_or_else(|| panic!("case {case:?}: an unauthorized invoke is refused"));
            assert_eq!(
                refused,
                format!("{sentence} {reason}"),
                "case {case:?}: got {refused:?}"
            );
        }
    }

    /// RULE: the account's own published credential holds no IAM policies, so it is not bounded
    /// by this gate.
    #[test]
    fn the_accounts_own_published_credential_is_not_bounded() {
        let mut caller = session(&[]);
        caller.account_credential = true;
        let refused = refusal(&caller, &call(&caller.arn.clone()), false);
        assert!(
            refused.is_none(),
            "case \"the account's own credential\": expected the invoke to be admitted, got \
             {refused:?}"
        );
    }
}
