// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this data plane perform the action it asks for — the
//! authorization step every operation runs BEFORE it resolves the job or the trajectory
//! the request names.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at what the
//! request addresses, so a caller whose identity-based policies do not grant the action is
//! told it is not authorized rather than told anything about the job it named, and the call
//! performs nothing. `AccessDeniedException` (HTTP 403, "You do not have permission to
//! perform this operation") is a SERVICE-COMMON error of this model
//! (`smithy_query(query="digest")` lists it under `common_errors`, and
//! `smithy_query(query="shape",shape="AccessDeniedException")` gives its status), so it is
//! the answer every one of the four operations gives such a caller — and it is the answer
//! BEFORE the job lookup, which is also what keeps a refusal from disclosing whether the
//! job exists.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)` under the request's condition
//!    context, so this service and every other one handed the same documents answer alike:
//!    an explicit `Deny` wins, otherwise an `Allow` about the request grants it, and with
//!    neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. That is
//! the split every other gate in this tree makes (Amazon Textract, AWS KMS, Amazon Bedrock,
//! Amazon OpenSearch Service, CloudWatch RUM), and the lookups stay in the service crate
//! for the reason those keep theirs there: hoisting them into `service-common` would make
//! every service depending on that crate link the IAM and STS service crates. WHICH IAM
//! ENTITY a principal ARN names carries no such dependency, so it is read through the
//! shared [`::service_common::iam_role`], and the account an ARN belongs to through
//! [`::service_common::arn`], rather than parsed again here.
//!
//! ## The action
//!
//! Each operation is its own IAM action, `sagemaker:<OperationName>` — this service's
//! `arnNamespace` (`smithy_query(query="service_info")`) followed by the operation's own
//! name — so a policy that withholds streaming inference while granting the non-streaming
//! one, or that grants only the rollout bookkeeping, decides each of the four calls
//! separately. Nothing here is keyed on which operation asked: the action is composed from
//! [`smithy_server_runtime::Operation::NAME`] at the one call site in each handler.
//!
//! ## The resource and the condition keys a statement is judged under
//!
//! Every operation of this model binds `JobArn`, whose `@pattern` carries the partition,
//! the Region and the 12-digit account the job belongs to, so the resource a request is
//! judged against is the ARN the REQUEST names — not one resolved from a stored record,
//! which is what keeps the decision independent of whether that job exists.
//!
//! A statement's `Condition` applies only for the keys the request can answer, and those
//! are the keys this gate publishes ([`request_conditions`]): the ARN of the principal that
//! made the request, the account that principal belongs to, and the account that owns the
//! resource it named. The last is what scopes a grant to the caller's own account — the
//! condition shape a cross-account `JobArn` fails — and it is read out of the request's own
//! ARN. Every other key is left unpublished, so a statement naming one stays undecided and
//! the shared evaluator reads it fail-closed.
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
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `sagemaker:<OperationName>`, from this
/// service's `arnNamespace` (`smithy_query(query="service_info")`), which is also the
/// vendor field of every `JobArn` it serves.
pub const ACTION_PREFIX: &str = "sagemaker";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The global condition keys that name the PRINCIPAL a request was signed by, which this
/// gate answers exactly: the ARN of the principal that made the request, and the account
/// that principal belongs to (IAM's global condition keys).
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// The global condition key that names the account OWNING the resource the request
/// addressed (IAM's global condition keys: the account of the resource being acted upon).
/// Its value is the account segment of the `JobArn` the request carried.
const RESOURCE_ACCOUNT_KEY: &str = "aws:ResourceAccount";

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the `JobArn` the request
/// NAMES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation returns as its
/// own `AccessDeniedException`. Call it FIRST in a handler, before the store is read or
/// written and before the request's own values are checked.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why every operation of this service may
/// take this decision.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    let conditions = request_conditions(&caller, resource);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this model binds no resource-based policy operation, so the
    // decision below rests on the action, the resource and the published condition keys.
    let request = iam::Request::new(&action, resource, &caller.principals)
        .with_context(&conditions.carried)
        .with_published_keys(&conditions.published);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal AWS states an authorization failure with, in the one of its two forms this
/// request falls under.
///
/// A request for a resource of the caller's OWN account is refused with IAM's
/// identity-based denial sentence: who was refused, which action, on which resource, and
/// which of IAM's rules refused it. A request for a resource of ANOTHER account is refused
/// with the sentence that names the principal alone — it states neither the action nor the
/// ARN, because a caller must learn nothing about a resource outside its own account, not
/// even which of them it asked about.
///
/// Every value in either sentence comes from the request and from the stored policies: the
/// principal from the credential the request presented, the action from the operation it
/// named, the resource from the ARN it addressed.
fn not_authorized(
    caller: &Caller,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    let message = if caller.addresses_another_account(resource) {
        format!("User: {} is not authorized to access this resource", caller.arn)
    } else {
        format!(
            "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
            caller.arn
        )
    };
    AccessDeniedException { message }
}

/// The account the ARN `arn` names, or `None` when it carries none — read through the
/// tree's one parse of a resource ARN, so an account is never guessed out of a value that
/// is no ARN at all.
fn account_of(arn: &str) -> Option<String> {
    ::service_common::arn::parse(arn)
        .map(|parsed| parsed.account)
        .filter(|account| !account.is_empty())
        .map(ToString::to_string)
}

/// The condition keys THIS request answers for, and the names it declares.
///
/// A key the request declares is one a clause can be DECIDED over — including deciding it
/// as absent — while a clause over any other key stays undecided and is read fail-closed.
/// The declared names are a fixed handful whatever the request carries, so deciding one
/// clause costs the same whatever the caller sent.
#[derive(Debug, Default)]
struct RequestConditions {
    /// The `(key, value)` pairs the request carries, one pair per value.
    carried: Vec<(String, String)>,
    /// The names the request models.
    published: Vec<&'static str>,
}

/// The condition context of one request: the principal that made it and the account owning
/// the resource it named.
///
/// A caller that speaks for nobody (an expired session credential) carries no principal
/// spelling, so there is nothing to compare and no principal key is declared; each account
/// is read out of the relevant ARN through the tree's one ARN parse, so an ARN carrying
/// none declares no account key rather than a guess.
fn request_conditions(caller: &Caller, resource: &str) -> RequestConditions {
    let mut conditions = RequestConditions::default();
    if !caller.principals.is_empty() {
        conditions
            .carried
            .push((PRINCIPAL_ARN_KEY.to_string(), caller.arn.clone()));
        conditions.published.push(PRINCIPAL_ARN_KEY);
        if let Some(account) = caller.account.clone() {
            conditions
                .carried
                .push((PRINCIPAL_ACCOUNT_KEY.to_string(), account));
            conditions.published.push(PRINCIPAL_ACCOUNT_KEY);
        }
    }
    if let Some(account) = account_of(resource) {
        conditions
            .carried
            .push((RESOURCE_ACCOUNT_KEY.to_string(), account));
        conditions.published.push(RESOURCE_ACCOUNT_KEY);
    }
    conditions
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The account that principal belongs to, when its ARN carries one.
    account: Option<String>,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Whether `resource` belongs to an account that is NOT this caller's.
    ///
    /// Both accounts have to be known for the comparison to say anything: a caller or a
    /// resource whose ARN carries no account is not treated as foreign.
    fn addresses_another_account(&self, resource: &str) -> bool {
        match (self.account.as_deref(), account_of(resource)) {
            (Some(principal), Some(owner)) => principal != owner,
            _ => false,
        }
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }

    /// The caller a request speaking for nobody is judged as: an expired session
    /// credential, which carries no document and no principal spelling, so every action it
    /// asks for falls to IAM's default deny.
    fn speaks_for_nobody() -> Self {
        Self {
            arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
            account: None,
            principals: Vec::new(),
            documents: Vec::new(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
        }
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // The identity this emulator's own published credential speaks for, which is both the
    // answer for a key that names no session and what tells such a request apart from one
    // signed by a principal the stores hold.
    let ambient = sts_store::ambient_identity();
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(ambient.clone()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The key id the expired credential presented is NOT quoted back — this module
        // echoes no credential of the request — and in the assembled emulator the layer
        // that authenticates a request refuses an expired credential before any handler
        // runs.
        Err(_expired) => return Caller::speaks_for_nobody(),
    };

    let mut caller = Caller {
        account_credential: identity.arn == ambient.arn,
        principals: vec![identity.arn.clone()],
        account: account_of(&identity.arn),
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
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.account = account_of(&user.arn);
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
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

    /// The account this emulator serves, which every ARN below is built in.
    fn account() -> String {
        ::engine::emulator_account_id().to_string()
    }

    /// A caller speaking for an assumed-role session of `account`.
    fn session_caller(account: &str) -> Caller {
        let arn = format!("arn:aws:sts::{account}:assumed-role/rft-runner/session-1");
        Caller {
            account: account_of(&arn),
            principals: vec![arn.clone()],
            arn,
            documents: Vec::new(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
        }
    }

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults, under the request's own condition context.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        let caller = session_caller(&account());
        let conditions = request_conditions(&caller, resource);
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &caller.principals)
                .with_context(&conditions.carried)
                .with_published_keys(&conditions.published),
        )
    }

    /// A `JobArn` of `account`, in the form this service's `@pattern` admits.
    fn job_arn(account: &str, name: &str) -> String {
        format!("arn:aws:sagemaker:us-east-1:{account}:job/AgentRFT/{name}")
    }

    /// RULE: the action asked about is `sagemaker:<OperationName>`, one per operation, so a
    /// policy scoped to one of this service's actions covers that operation alone and a
    /// policy scoped to the namespace covers all four.
    #[test]
    fn each_operation_is_asked_about_under_its_own_action() {
        let operations = [
            "Sample",
            "SampleWithResponseStream",
            "CompleteRollout",
            "UpdateReward",
        ];
        let resource = job_arn(&account(), "run-1");
        for granted in operations {
            let document = format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"{ACTION_PREFIX}:{granted}",
                 "Resource":"*"}}]}}"#
            );
            for asked in operations {
                let action = format!("{ACTION_PREFIX}:{asked}");
                let reached = decision(&document, &action, &resource);
                let expected = if asked == granted {
                    Decision::Allowed
                } else {
                    Decision::NotStated
                };
                assert_eq!(
                    reached, expected,
                    "case a grant of {granted:?} asked about {asked:?}: expected {expected:?}, \
                     got {reached:?}"
                );
            }
        }

        let namespace =
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"{ACTION_PREFIX}:*","Resource":"*"}}]}}"#);
        for asked in operations {
            let action = format!("{ACTION_PREFIX}:{asked}");
            let reached = decision(&namespace, &action, &resource);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case the whole namespace asked about {asked:?}: expected Allowed, got {reached:?}"
            );
        }
        let outside = decision(&namespace, "s3:GetObject", &resource);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected this namespace to grant nothing outside it, got \
             {outside:?}"
        );
    }

    /// RULE: the request publishes the account that owns the resource it named, so a grant
    /// conditioned on it holding the principal's own account covers a `JobArn` of that
    /// account and nothing of another.
    #[test]
    fn the_resource_account_is_published_for_a_condition_to_decide() {
        let mine = account();
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"{ACTION_PREFIX}:Sample",
             "Resource":"arn:aws:sagemaker:*:*:job/*",
             "Condition":{{"StringEquals":{{"{RESOURCE_ACCOUNT_KEY}":
             "${{{PRINCIPAL_ACCOUNT_KEY}}}"}}}}}}]}}"#
        );
        let action = format!("{ACTION_PREFIX}:Sample");
        // Rows differ along the account the resource belongs to: the caller's own, two
        // other well-formed accounts, and a value that is no ARN at all.
        let rows: &[(&str, String, Decision)] = &[
            (
                "the caller's own account",
                job_arn(&mine, "run-1"),
                Decision::Allowed,
            ),
            (
                "another account",
                job_arn("999999999999", "run-1"),
                Decision::NotStated,
            ),
            (
                "an account differing in one digit",
                job_arn("000000000001", "run-1"),
                Decision::NotStated,
            ),
            (
                "a value that is no ARN, so no resource account is published",
                "AgentRFT/run-1".to_string(),
                Decision::NotStated,
            ),
        ];
        for (case, resource, expected) in rows {
            let reached = decision(&document, &action, resource);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?} for {resource}, got {reached:?}"
            );
        }
    }

    /// RULE: a refusal about a resource of the caller's own account names the principal,
    /// the action, that resource and which of IAM's rules refused; one about a resource of
    /// ANOTHER account names the principal alone.
    #[test]
    fn the_refusal_wording_follows_the_account_owning_the_resource() {
        let mine = account();
        let caller = session_caller(&mine);
        let action = format!("{ACTION_PREFIX}:Sample");
        let reason = format!("because no identity-based policy allows the {action} action");

        let own = job_arn(&mine, "run-1");
        let stated = not_authorized(&caller, &action, &own, &reason).message;
        assert_eq!(
            stated,
            format!(
                "User: {} is not authorized to perform: {action} on resource: {own} {reason}",
                caller.arn
            ),
            "case \"the caller's own account\": expected IAM's identity-based denial, got \
             {stated:?}"
        );

        for (case, foreign) in [
            ("another account", job_arn("999999999999", "run-1")),
            ("an account differing in one digit", job_arn("000000000001", "run-1")),
        ] {
            let stated = not_authorized(&caller, &action, &foreign, &reason).message;
            assert_eq!(
                stated,
                format!("User: {} is not authorized to access this resource", caller.arn),
                "case {case:?}: expected the refusal to name the principal alone, got {stated:?}"
            );
            assert!(
                !stated.contains(&foreign) && !stated.contains(&action),
                "case {case:?}: a cross-account refusal must name neither the resource nor the \
                 action, got {stated:?}"
            );
        }
    }

    /// One row of the principal-reading table: an ARN a request can speak for, and what the
    /// shared readings must make of it.
    struct PrincipalRow {
        case: &'static str,
        arn: String,
        /// The IAM role a session ARN names, whose policies the gate reads.
        role: Option<&'static str>,
        /// The IAM user the ARN names, whose policies the gate reads.
        user: Option<&'static str>,
        /// The account the ARN carries, which the condition context publishes.
        owner: Option<String>,
    }

    /// RULE: a principal ARN is read through the SHARED readings of an identity ARN, so a
    /// session ARN names the role whose policies this gate reads and an IAM user ARN names
    /// the user whose policies it reads, while every other principal form names neither.
    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        let account = account();
        let rows = vec![
            PrincipalRow {
                case: "an assumed-role session",
                arn: format!("arn:aws:sts::{account}:assumed-role/rft-runner/session-1"),
                role: Some("rft-runner"),
                user: None,
                owner: Some(account.clone()),
            },
            PrincipalRow {
                case: "a session name carrying a slash",
                arn: format!("arn:aws:sts::{account}:assumed-role/rft-runner/one/two"),
                role: Some("rft-runner"),
                user: None,
                owner: Some(account.clone()),
            },
            PrincipalRow {
                case: "an IAM user",
                arn: format!("arn:aws:iam::{account}:user/rft-user"),
                role: None,
                user: Some("rft-user"),
                owner: Some(account.clone()),
            },
            PrincipalRow {
                case: "an IAM user under a path",
                arn: format!("arn:aws:iam::{account}:user/team/rft-user"),
                role: None,
                user: Some("rft-user"),
                owner: Some(account.clone()),
            },
            PrincipalRow {
                case: "the account's own identity",
                arn: format!("arn:aws:iam::{account}:root"),
                role: None,
                user: None,
                owner: Some(account.clone()),
            },
            PrincipalRow {
                case: "a value that is no ARN",
                arn: "AKIDTESTONLY".to_string(),
                role: None,
                user: None,
                owner: None,
            },
        ];
        for PrincipalRow {
            case,
            arn,
            role,
            user,
            owner,
        } in rows
        {
            let read_role = ::service_common::iam_role::assumed_role_session_role_name(&arn);
            let read_user = ::service_common::iam_role::user_name(&arn);
            let read_account = account_of(&arn);
            assert_eq!(
                read_role, role,
                "case {case:?}: expected the role {role:?}, got {read_role:?}"
            );
            assert_eq!(
                read_user, user,
                "case {case:?}: expected the user {user:?}, got {read_user:?}"
            );
            assert_eq!(
                read_account, owner,
                "case {case:?}: expected the account {owner:?}, got {read_account:?}"
            );
        }
    }
}
