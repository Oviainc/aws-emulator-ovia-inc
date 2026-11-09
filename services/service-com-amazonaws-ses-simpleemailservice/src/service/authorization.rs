// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service SEND the message it carries — the
//! authorization step every send runs BEFORE the handler looks at the account's
//! identities.
//!
//! Amazon SES is an IAM-authorized service, and AWS decides authorization at the IAM
//! layer AHEAD of any service-level validation of the request: a caller whose
//! identity-based policies do not allow the sending action for the message it is sending
//! is told it is not authorized, rather than told anything about the identity it named,
//! and the call changes nothing. That is why the refusal here is HTTP 403 `AccessDenied`
//! — the authorization layer's own answer, which no operation of this model declares as an
//! error — and why it is answered before the sender gate that refuses an unverified
//! identity with `MessageRejected`.
//!
//! Each send takes this decision before ANYTHING the account holds is read, and after
//! only one thing: the request-shape ceiling on its recipient set. The decision is taken
//! over every recipient the request names (`ses:Recipients` carries one value per
//! recipient), so that ceiling is what bounds how much of a caller's request the decision
//! has to read; every other check of the message follows the decision.
//!
//! What makes this more than a yes to every signed request is that a sending statement can
//! be scoped by the condition keys SES publishes for a send, which is the tenancy control
//! SES documents for the purpose ("Restricting the From address", in the Developer Guide's
//! chapter on controlling user access): a credential allowed `ses:SendEmail` only under
//! `StringEquals {"ses:FromAddress": […]}` can send as the addresses that condition names
//! and as no other. The keys this gate resolves from the request are the ones the IAM
//! Service Authorization Reference lists for this service's email-sending actions
//! ([`SENDING_CONDITION_KEYS`]), each taken from the message being sent — so a policy
//! written against any of them is DECIDED rather than left undecided, which (the shared
//! evaluator reads an undecided clause fail-closed) would otherwise refuse a send AWS
//! permits.
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
//!    where the collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes
//!    effect on the next send and widening it lifts the refusal on the one after;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against `(action, resource)` under a request's
//!    condition context, so this service and every other one handed the same documents
//!    answer alike: an explicit `Deny` wins, otherwise an `Allow` about the request grants
//!    it, and with neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. The
//! lookups stay in this service crate for the reason every other service's own
//! `authorization` module records: hoisting them into `service-common` would make every
//! service that depends on that crate link the IAM and STS service crates.
//!
//! The RESOURCE a send is judged against is the identity the message is sent FROM —
//! `arn:aws:ses:<region>:<account>:identity/<from address>` — built from what the request
//! carried rather than from a stored record, so the decision does not depend on whether
//! that identity exists, which is also what keeps a refused send from telling an
//! unauthorized caller whether it does. The sending-authorization policies an identity of
//! this account carries are a different document in a different direction (a resource-based
//! policy, written by `PutIdentityPolicy`, which grants ANOTHER account's principals the
//! use of an identity); this gate decides the caller's OWN permission, and the two are not
//! merged here.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no user can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every send it asks for is refused. (A session the authenticating
//! layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `ses:<OperationName>` ("Actions defined
/// by Amazon SES", IAM Service Authorization Reference), which is this service's
/// `arnNamespace` (`smithy_query(query="service_info")`) — the same string every ARN this
/// service mints spells.
pub const ACTION_PREFIX: &str = "ses";

/// The resource type an SES identity ARN addresses, which is the resource a send is judged
/// against: `arn:aws:ses:<region>:<account>:identity/<identity>` (the form every
/// `…Arn` member of this model documents).
const IDENTITY_RESOURCE_TYPE: &str = "identity";

/// The condition key naming the address a message is sent FROM: "ses:FromAddress — the
/// 'From' address of a message" (IAM Service Authorization Reference, condition keys for
/// Amazon SES). This is the key AWS's "Restricting the From address" lever is written
/// against.
pub const FROM_ADDRESS_KEY: &str = "ses:FromAddress";

/// The condition key naming the display name in front of that address
/// (`ses:FromDisplayName`), which a `From` value may carry and a bare address does not.
pub const FROM_DISPLAY_NAME_KEY: &str = "ses:FromDisplayName";

/// The condition key naming the message's recipients (`ses:Recipients`) — one value per
/// recipient, so a statement over it is decided under IAM's set qualifiers.
pub const RECIPIENTS_KEY: &str = "ses:Recipients";

/// The condition key naming where bounces and complaints are forwarded
/// (`ses:FeedbackAddress`), which is the `ReturnPath` a send carries.
pub const FEEDBACK_ADDRESS_KEY: &str = "ses:FeedbackAddress";

/// The condition keys this gate resolves from the message being sent, which are the ones
/// the IAM Service Authorization Reference lists for this service's email-sending actions.
///
/// Declared to the evaluator as the keys the request MODELS, so a key the message carries
/// no value for is read as absent rather than as a key this emulator cannot answer for;
/// every OTHER key stays unpublished, which leaves a statement naming one undecided and
/// therefore read fail-closed.
pub const SENDING_CONDITION_KEYS: &[&str] = &[
    FROM_ADDRESS_KEY,
    FROM_DISPLAY_NAME_KEY,
    RECIPIENTS_KEY,
    FEEDBACK_ADDRESS_KEY,
];

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The message a send asks to put on the wire, as the condition keys of a sending
/// statement see it.
///
/// Built from the REQUEST — every value here is one the caller supplied — because that is
/// what IAM decides a send against, and because authorization runs before anything the
/// account holds is read.
#[derive(Debug, Clone, Default)]
pub struct SendingRequest {
    /// The addr-spec the message is sent from: `ses:FromAddress`, and the identity the
    /// send is judged against.
    pub from_address: String,
    /// The display name in front of that address, when the value carried one:
    /// `ses:FromDisplayName`.
    pub from_display_name: Option<String>,
    /// Every recipient's addr-spec: the values of `ses:Recipients`.
    pub recipients: Vec<String>,
    /// The address bounces and complaints are forwarded to, when the send named one:
    /// `ses:FeedbackAddress`.
    pub feedback_address: Option<String>,
}

impl SendingRequest {
    /// The message a send carries, from the values the request supplied for it: the
    /// `Source`/`From` value (bare, or behind a display name), the recipient list, and the
    /// `ReturnPath` when it named one.
    ///
    /// Each address is read down to its addr-spec the way every other reader of a
    /// caller-supplied address on this surface reads one
    /// ([`crate::state::addr_spec`]), so the value a condition is compared against is the
    /// mailbox the message is really sent from rather than the decoration around it.
    #[must_use]
    pub fn new(source: &str, recipients: &[String], return_path: Option<&str>) -> Self {
        Self {
            from_address: crate::state::addr_spec(source).to_string(),
            from_display_name: display_name(source),
            recipients: recipients
                .iter()
                .map(|recipient| crate::state::addr_spec(recipient).to_string())
                .collect(),
            feedback_address: return_path
                .map(|address| crate::state::addr_spec(address).to_string()),
        }
    }

    /// The condition context this message puts the statement's clauses to: one `(key,
    /// value)` pair per key the message carries a value for, and one pair per recipient
    /// for the multi-valued key.
    fn condition_context(&self) -> Vec<(String, String)> {
        let mut carried = vec![(FROM_ADDRESS_KEY.to_string(), self.from_address.clone())];
        if let Some(display_name) = &self.from_display_name {
            carried.push((FROM_DISPLAY_NAME_KEY.to_string(), display_name.clone()));
        }
        for recipient in &self.recipients {
            carried.push((RECIPIENTS_KEY.to_string(), recipient.clone()));
        }
        if let Some(feedback_address) = &self.feedback_address {
            carried.push((FEEDBACK_ADDRESS_KEY.to_string(), feedback_address.clone()));
        }
        carried
    }
}

/// The display name in front of an address value, when it carries one: `"Me"
/// <me@example.com>` is sent by `me@example.com` under the display name `Me`.
///
/// Read with the RFC 5322 address reader this crate already parses messages with
/// (`mailparse`), rather than by cutting the string here: an address header is that
/// grammar, and a value with no display name — or one this reader cannot read as an
/// address at all — carries none.
fn display_name(value: &str) -> Option<String> {
    let parsed = ::mailparse::addrparse(value).ok()?;
    parsed.iter().find_map(|entry| match entry {
        ::mailparse::MailAddr::Single(single) => single.display_name.clone(),
        ::mailparse::MailAddr::Group(_) => None,
    })
}

/// Authorize this send, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `send` the message the request carries.
///
/// `Ok(())` for a send IAM permits (and for every caller this gate does not bound, see the
/// module docs); otherwise the authorization layer's refusal — HTTP 403 `AccessDenied` —
/// which the handler returns before it reads the account's identities.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called.
pub fn authorize_send<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    region: &str,
    send: &SendingRequest,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let resource = identity_arn(region, &send.from_address);
    let caller = caller(engine, ctx);
    let context = send.condition_context();
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action, the resource and the
    // condition keys the message carries.
    let request = iam::Request::new(&action, &resource, &caller.principals)
        .with_context(&context)
        .with_published_keys(SENDING_CONDITION_KEYS);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the send whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(&caller.arn, &action, &resource));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(&caller.arn, &action, &resource));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        if iam::identity_decision(::std::slice::from_ref(boundary), &request) != Decision::Allowed {
            return Err(not_authorized(&caller.arn, &action, &resource));
        }
    }
    Ok(())
}

/// The ARN of the email identity a send from `from_address` is judged against, in the
/// Region the request is scoped to.
///
/// Built through the account contract's own ARN builder, so the partition and the account
/// segment are the ones every other ARN this emulator emits carry.
#[must_use]
pub fn identity_arn(region: &str, from_address: &str) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        region,
        &format!("{IDENTITY_RESOURCE_TYPE}/{from_address}"),
    )
}

/// The refusal the authorization layer answers an unauthorized send with: who was refused,
/// which action, and the identity resource the send addressed.
///
/// Every value in it comes from the request and from the stored policies — the principal
/// from the credential the request presented, the action from the operation it named, the
/// resource from the address the message is sent from. The sentence states no reason
/// clause: this surface's refusal names the three facts and nothing else.
fn not_authorized<E>(
    principal: &str,
    action: &str,
    resource: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::access_denied(format!(
        "User '{principal}' is not authorized to perform '{action}' on resource '{resource}'"
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
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
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
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
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every send it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no credential of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
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
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
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

    /// The decision one stored document reaches about a send, through the shared evaluator
    /// this gate consults and under the condition context the message carries.
    fn decision(document: &str, action: &str, resource: &str, send: &SendingRequest) -> Decision {
        let context = send.condition_context();
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[])
                .with_context(&context)
                .with_published_keys(SENDING_CONDITION_KEYS),
        )
    }

    /// A send from `source` to one recipient.
    fn send_from(source: &str) -> SendingRequest {
        SendingRequest::new(
            source,
            &["success@simulator.amazonses.com".to_string()],
            None,
        )
    }

    /// RULE: a statement scoped by `ses:FromAddress` covers the addresses it names and no
    /// other — whichever sending action it allows — so the key is answered from the
    /// message rather than left undecided.
    #[test]
    fn a_from_address_condition_decides_which_sources_are_granted() {
        let document = r#"{"Statement":[{"Effect":"Allow",
            "Action":["ses:SendEmail","ses:SendRawEmail","ses:SendTemplatedEmail",
                      "ses:SendBulkTemplatedEmail"],
            "Resource":"*",
            "Condition":{"StringEquals":{"ses:FromAddress":["allowed@example.com"]}}}]}"#;
        for operation in [
            "SendEmail",
            "SendRawEmail",
            "SendTemplatedEmail",
            "SendBulkTemplatedEmail",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            // (case, the From value the message carries, the decision expected)
            let rows: &[(&str, &str, Decision)] = &[
                (
                    "the address the condition names",
                    "allowed@example.com",
                    Decision::Allowed,
                ),
                (
                    "the same address behind a display name",
                    "\"Sender\" <allowed@example.com>",
                    Decision::Allowed,
                ),
                (
                    "an address the condition does not name",
                    "denied@example.com",
                    Decision::NotStated,
                ),
                (
                    "another address of the same domain",
                    "someone.else@example.com",
                    Decision::NotStated,
                ),
            ];
            for (case, source, expected) in rows {
                let send = send_from(source);
                let reached = decision(
                    document,
                    &action,
                    &identity_arn("us-west-2", &send.from_address),
                    &send,
                );
                assert_eq!(
                    reached, *expected,
                    "case {operation} / {case:?}: expected {expected:?} for {source:?}, \
                     got {reached:?}"
                );
            }
        }
    }

    /// RULE: the resource a send is judged against is the identity ARN of the address the
    /// message is sent from, so a statement scoped to one identity's ARN covers sends from
    /// that address alone.
    #[test]
    fn the_resource_is_the_identity_arn_of_the_from_address() {
        let region = "eu-west-1";
        let arn = identity_arn(region, "allowed@example.com");
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"ses:SendEmail","Resource":"{arn}"}}]}}"#
        );
        assert_eq!(
            arn,
            format!(
                "arn:aws:ses:{region}:{}:identity/allowed@example.com",
                ::engine::EMULATOR_ACCOUNT_ID
            ),
            "case the resource ARN: expected the published identity ARN form, got {arn:?}"
        );
        // (case, the From value, the decision expected)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "the identity the statement names",
                "allowed@example.com",
                Decision::Allowed,
            ),
            (
                "another identity of the account",
                "other@example.com",
                Decision::NotStated,
            ),
        ];
        for (case, source, expected) in rows {
            let send = send_from(source);
            let reached = decision(
                &document,
                "ses:SendEmail",
                &identity_arn(region, &send.from_address),
                &send,
            );
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?} for {source:?}, got {reached:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the sending action and the identity resource
    /// the send addressed — every part taken from the request and the caller's credential.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_identity() {
        let principal = format!(
            "arn:aws:iam::{}:user/scoped-sender",
            ::engine::EMULATOR_ACCOUNT_ID
        );
        let resource = identity_arn("us-west-2", "denied@example.com");
        let refused: crate::scaffolded::Scaffolded<()> =
            not_authorized(&principal, "ses:SendEmail", &resource);
        let message = match &refused {
            crate::scaffolded::Scaffolded::AccessDenied(payload) => payload.message.clone(),
            other => panic!("case the refusal: expected AccessDenied, got {other:?}"),
        };
        assert_eq!(
            message,
            format!(
                "User '{principal}' is not authorized to perform 'ses:SendEmail' on resource \
                 '{resource}'"
            ),
            "case the refusal: expected the authorization layer's sentence, got {message:?}"
        );
    }

    /// RULE: every sending condition key this gate publishes is answered from the message,
    /// so a statement scoped by any of them is decided rather than fail-closed.
    #[test]
    fn every_published_condition_key_is_answered_from_the_message() {
        let send = SendingRequest::new(
            "\"Marketing\" <sender@example.com>",
            &["one@example.com".to_string(), "two@example.com".to_string()],
            Some("bounces@example.com"),
        );
        let context = send.condition_context();
        // (the key, the values the message carries for it)
        let expected: &[(&str, Vec<&str>)] = &[
            (FROM_ADDRESS_KEY, vec!["sender@example.com"]),
            (FROM_DISPLAY_NAME_KEY, vec!["Marketing"]),
            (RECIPIENTS_KEY, vec!["one@example.com", "two@example.com"]),
            (FEEDBACK_ADDRESS_KEY, vec!["bounces@example.com"]),
        ];
        for (key, values) in expected {
            let carried: Vec<&str> = context
                .iter()
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(
                &carried, values,
                "case {key:?}: expected {values:?} from the message, got {carried:?}"
            );
        }
        // A key the message carries no value for is absent from the context rather than
        // carried empty: a bare `From` has no display name and a send with no `ReturnPath`
        // names no feedback address.
        let bare = SendingRequest::new("sender@example.com", &[], None);
        let bare_context = bare.condition_context();
        let carried: Vec<&str> = bare_context
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            carried,
            vec![FROM_ADDRESS_KEY],
            "case a bare From with no recipients: expected only the From address, got {carried:?}"
        );
    }
}
