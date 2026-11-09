// SPDX-License-Identifier: CC0-1.0

//! The IAM AUTHORIZATION of a send: which requests the calling principal's own policies
//! let this service put on the wire, over the awsQuery wire.
//!
//! THE RULE: Amazon SES is an IAM-authorized service and authorization runs BEFORE any
//! SES-level validation of the message. Every send is decided against the identity-based
//! policies in force on the principal the request was signed by, under the sending
//! condition keys taken from the message itself (`ses:FromAddress`,
//! `ses:FromDisplayName`, `ses:Recipients`, `ses:FeedbackAddress`) and against the
//! identity resource the message is sent from
//! (`arn:aws:ses:<region>:<account>:identity/<from address>`); a send no policy allows is
//! refused HTTP 403 `AccessDenied`, deterministically, naming the principal, the
//! `ses:<OperationName>` action and that resource — and the sender gate that answers
//! `MessageRejected` for an unverified identity is reached only once authorization has
//! passed.
//!
//! The principals below are made with IAM's OWN operations over the same engine —
//! `CreateUser` + `PutUserPolicy` + `CreateAccessKey` — so the documents this service
//! reads are the ones IAM wrote, and the decision is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! The tables span what the rule generalises over:
//!
//! * the SOURCE a scoped credential sends from — the address its condition names, the
//!   same address behind a display name, and addresses it does not name;
//! * the OPERATION — `SendEmail`, `SendRawEmail`, `SendTemplatedEmail` and
//!   `SendBulkTemplatedEmail`, each judged under its own `ses:` action;
//! * the DOCUMENT in force — none at all, an unconditional allow, an allow scoped by each
//!   of the four sending condition keys, an allow scoped to one identity's ARN, an allow
//!   for another Region's ARN, and an explicit deny beside an allow;
//! * the KIND of caller — an IAM user signing with a long-term key, and the account's own
//!   published credential, which no identity policy governs;
//! * what happens when the policy is WIDENED, which is what proves the document is really
//!   read on every call rather than once.

use std::sync::Arc;

use service_com_amazonaws_ses_simpleemailservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    element, error_envelope, field, form_encode, settle_verifications, xml_text, Field, VERSION,
};

/// The Region every call in this suite is signed for, and therefore the Region of the
/// identity ARN a send is judged against.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the account's ambient identity, which no
/// identity policy governs — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// A domain reserved for documentation and examples (RFC 2606), so no row can reach a
/// real mailbox: the two addresses below are of that domain.
///
/// The two addresses a scoped credential is tested against: the one its condition names,
/// and one it does not.
const ALLOWED: &str = "allowed@authz.example.com";
const DENIED: &str = "denied@authz.example.com";

/// The mailbox simulator address a send addresses, which is a recipient AWS documents for
/// exactly this purpose.
const RECIPIENT: &str = "success@simulator.amazonses.com";

/// Another recipient, for the rows that vary `ses:Recipients`.
const OTHER_RECIPIENT: &str = "bounce@simulator.amazonses.com";

/// The IAM API version an awsQuery call to IAM carries.
const IAM_VERSION: &str = "2010-05-08";

/// The error code the IAM authorization layer refuses an unauthorized request with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The error code the SES sender gate refuses an unverified identity with, which is what
/// a send that PASSED authorization is answered with here.
const MESSAGE_REJECTED: &str = "MessageRejected";

/// This emulator: one shared engine carrying this service beside the IAM service the
/// principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One awsQuery call to `signing_name`, signed with `access_key_id` in the SigV4
    /// credential scope — which is both how the router resolves the service and where a
    /// handler reads the caller's key id from.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        access_key_id: &str,
        action: &str,
        params: &[Field],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("http://localhost/")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).into_owned(),
        )
    }

    /// One call to THIS service, signed with `access_key_id`.
    async fn ses(&self, access_key_id: &str, action: &str, params: &[Field]) -> (u16, String) {
        self.query("ses", VERSION, access_key_id, action, params)
            .await
    }

    /// One call to IAM, made with the account's own credential.
    async fn iam(&self, action: &str, params: &[Field]) -> (u16, String) {
        let (status, body) = self
            .query("iam", IAM_VERSION, ACCOUNT_KEY, action, params)
            .await;
        assert_eq!(
            status, 200,
            "case IAM {action}: expected 200, got {status} with {body}"
        );
        (status, body)
    }

    /// Create an IAM user carrying `policy` as its inline policy (when one is given) and
    /// answer the access key id of a long-term key minted for it, with the user's ARN —
    /// both read out of IAM's own answers rather than rebuilt here.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> (String, String) {
        let (_, body) = self
            .iam("CreateUser", &[field("UserName", user_name)])
            .await;
        let arn = element(&body, "Arn")
            .unwrap_or_else(|| panic!("CreateUser must answer the user Arn, got {body}"));
        if let Some(policy) = policy {
            self.put_user_policy(user_name, policy).await;
        }
        let (_, body) = self
            .iam("CreateAccessKey", &[field("UserName", user_name)])
            .await;
        let key = element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"));
        (key, arn)
    }

    /// Write (or rewrite) the inline policy of `user_name`, which is how a row WIDENS what
    /// a credential may send.
    async fn put_user_policy(&self, user_name: &str, policy: &str) {
        self.iam(
            "PutUserPolicy",
            &[
                field("UserName", user_name),
                field("PolicyName", "sending"),
                field("PolicyDocument", policy),
            ],
        )
        .await;
    }

    /// `VerifyEmailIdentity` for `address`, settled past the span the request leaves the
    /// record pending for — what a row needs when it is about an ACCEPTED send rather than
    /// about the span.
    async fn verify(&self, address: &str) {
        let (status, body) = self
            .ses(ACCOUNT_KEY, "VerifyEmailIdentity", &[field("EmailAddress", address)])
            .await;
        assert_eq!(
            status, 200,
            "case verify {address:?}: expected 200, got {status} with {body}"
        );
        settle_verifications(&self.engine);
    }

    /// `GetSendQuota`'s `SentLast24Hours`, which is DERIVED from the send log: how a row
    /// reads back what an accepted send wrote.
    async fn sent_last_24_hours(&self) -> f64 {
        let (status, body) = self.ses(ACCOUNT_KEY, "GetSendQuota", &[]).await;
        assert_eq!(
            status, 200,
            "case GetSendQuota: expected 200, got {status} with {body}"
        );
        element(&body, "SentLast24Hours")
            .unwrap_or_else(|| panic!("GetSendQuota must answer SentLast24Hours, got {body}"))
            .parse::<f64>()
            .expect("SentLast24Hours is a number")
    }
}

/// The ARN of the email identity a send from `address` is judged against, in `region`.
fn identity_arn(region: &str, address: &str) -> String {
    engine::emulator_arn("ses", region, &format!("identity/{address}"))
}

/// The sentence the authorization layer refuses an unauthorized send with.
fn not_authorized(principal: &str, operation: &str, address: &str) -> String {
    format!(
        "User '{principal}' is not authorized to perform 'ses:{operation}' on resource '{}'",
        identity_arn(REGION, address)
    )
}

/// An identity policy document carrying one statement.
fn policy(statement: &str) -> String {
    format!(r#"{{"Version":"2012-10-17","Statement":[{statement}]}}"#)
}

/// An `Allow` of every send this suite makes, on every resource, scoped by `condition`
/// when the row gives one.
fn allow_sending(condition: Option<&str>) -> String {
    let condition = condition.map_or_else(String::new, |clause| format!(r#","Condition":{clause}"#));
    policy(&format!(
        r#"{{"Effect":"Allow","Action":["ses:SendEmail","ses:SendRawEmail",
             "ses:SendTemplatedEmail","ses:SendBulkTemplatedEmail"],
           "Resource":"*"{condition}}}"#
    ))
}

/// A `StringEquals` clause over one condition key and the values it names.
fn string_equals(key: &str, values: &[&str]) -> String {
    let values = values
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"StringEquals":{{"{key}":[{values}]}}}}"#)
}

/// Which send a row makes. Every variant addresses [`RECIPIENT`] unless the row says
/// otherwise, so what differs between them is the operation and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sending {
    Email,
    RawEmail,
    TemplatedEmail,
    BulkTemplatedEmail,
}

impl Sending {
    /// The operation name, which is also the `ses:` action it is authorized under.
    fn operation(self) -> &'static str {
        match self {
            Self::Email => "SendEmail",
            Self::RawEmail => "SendRawEmail",
            Self::TemplatedEmail => "SendTemplatedEmail",
            Self::BulkTemplatedEmail => "SendBulkTemplatedEmail",
        }
    }

    /// The form fields that carry a send from `source` to `recipients` on the wire, with
    /// `return_path` when the row names one.
    fn fields(self, source: &str, recipients: &[&str], return_path: Option<&str>) -> Vec<Field> {
        let mut params = Vec::new();
        match self {
            Self::Email => {
                params.push(field("Source", source));
                for (index, address) in recipients.iter().enumerate() {
                    params.push(field(
                        format!("Destination.ToAddresses.member.{}", index + 1),
                        *address,
                    ));
                }
                params.push(field("Message.Subject.Data", "Quarterly report"));
                params.push(field("Message.Body.Text.Data", "See attached."));
            }
            Self::RawEmail => {
                // A raw send states its `From` in the message itself, which is where the
                // authorization layer reads the From address of such a call from.
                let mut raw = format!("From: {source}\r\nTo: {}\r\n", recipients.join(", "));
                if let Some(return_path) = return_path {
                    raw.push_str(&format!("Return-Path: {return_path}\r\n"));
                }
                raw.push_str("Subject: Quarterly report\r\n\r\nSee attached.\r\n");
                params.push(field(
                    "RawMessage.Data",
                    ::aws_smithy_types::base64::encode(&raw),
                ));
                // No `Source` parameter and no `Destinations`: both are read off the
                // message, which is the leg the gate has to cope with.
                return params;
            }
            Self::TemplatedEmail => {
                params.push(field("Source", source));
                for (index, address) in recipients.iter().enumerate() {
                    params.push(field(
                        format!("Destination.ToAddresses.member.{}", index + 1),
                        *address,
                    ));
                }
                params.push(field("Template", "quarterly"));
                params.push(field("TemplateData", "{}"));
            }
            Self::BulkTemplatedEmail => {
                params.push(field("Source", source));
                params.push(field("Template", "quarterly"));
                params.push(field("DefaultTemplateData", "{}"));
                for (index, address) in recipients.iter().enumerate() {
                    params.push(field(
                        format!(
                            "Destinations.member.1.Destination.ToAddresses.member.{}",
                            index + 1
                        ),
                        *address,
                    ));
                }
            }
        }
        if let Some(return_path) = return_path {
            params.push(field("ReturnPath", return_path));
        }
        params
    }
}

/// What a send was answered with: the HTTP status and the envelope's `<Code>`.
struct Answer {
    status: u16,
    code: String,
    message: String,
}

/// Make `send` as `access_key_id` and read the answer's envelope.
async fn send_as(
    ses: &Emulator,
    access_key_id: &str,
    send: Sending,
    source: &str,
    recipients: &[&str],
    return_path: Option<&str>,
) -> Answer {
    let (status, body) = ses
        .ses(
            access_key_id,
            send.operation(),
            &send.fields(source, recipients, return_path),
        )
        .await;
    let (code, message) = error_envelope(&body);
    Answer {
        status,
        code: code.unwrap_or_default(),
        message: message.unwrap_or_default(),
        // The success body is not an error envelope, so neither element is there; a row
        // that expects a success asserts on the status and the stored send instead.
    }
}

/// The three attempts the rule's "deterministically" half is asserted over: a policy
/// decision is taken on every call, so the same unauthorized send is refused the same way
/// however often it is made.
const ATTEMPTS: usize = 3;

#[tokio::test]
async fn a_source_the_from_address_condition_does_not_name_is_not_authorized() {
    let ses = Emulator::new();
    let user_name = "scoped-sender";
    let (key, principal) = ses
        .user(
            user_name,
            Some(&allow_sending(Some(&string_equals(
                "ses:FromAddress",
                &[ALLOWED],
            )))),
        )
        .await;

    // The address the condition does NOT name is refused by authorization — on every
    // attempt, with the authorization layer's own status, code and sentence.
    for attempt in 1..=ATTEMPTS {
        let answer = send_as(&ses, &key, Sending::Email, DENIED, &[RECIPIENT], None).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (403, ACCESS_DENIED),
            "case attempt {attempt} from {DENIED}: expected 403 {ACCESS_DENIED}, got {} {} {:?}",
            answer.status,
            answer.code,
            answer.message
        );
        assert_eq!(
            answer.message,
            not_authorized(&principal, "SendEmail", DENIED),
            "case attempt {attempt} from {DENIED}: expected the authorization sentence, got {:?}",
            answer.message
        );
    }

    // The address it DOES name passes authorization and is answered by the sender gate,
    // which is the next thing a send meets.
    let answer = send_as(&ses, &key, Sending::Email, ALLOWED, &[RECIPIENT], None).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, MESSAGE_REJECTED),
        "case from {ALLOWED}: expected the sender gate to answer, got {} {} {:?}",
        answer.status,
        answer.code,
        answer.message
    );

    // Widening the condition to name BOTH addresses lifts the refusal on the very next
    // call: the document is read on every send, not once.
    ses.put_user_policy(
        user_name,
        &allow_sending(Some(&string_equals("ses:FromAddress", &[ALLOWED, DENIED]))),
    )
    .await;
    let answer = send_as(&ses, &key, Sending::Email, DENIED, &[RECIPIENT], None).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, MESSAGE_REJECTED),
        "case from {DENIED} once the condition names it: expected the sender gate to answer, \
         got {} {} {:?}",
        answer.status,
        answer.code,
        answer.message
    );

    // And nothing was sent: an unauthorized send writes nothing, and the two sends that
    // passed authorization were refused by the sender gate.
    let sent = ses.sent_last_24_hours().await;
    assert_eq!(
        sent, 0.0,
        "case the send log: a send no identity gate admitted must log nothing, got {sent}"
    );
}

#[tokio::test]
async fn each_sending_operation_is_authorized_under_its_own_action() {
    let ses = Emulator::new();
    // A policy allowing ONE sending action and nothing else, so each operation is shown
    // to be judged under its own.
    let (key, principal) = ses
        .user(
            "one-action-sender",
            Some(&policy(
                r#"{"Effect":"Allow","Action":"ses:SendEmail","Resource":"*"}"#,
            )),
        )
        .await;
    for send in [
        Sending::Email,
        Sending::RawEmail,
        Sending::TemplatedEmail,
        Sending::BulkTemplatedEmail,
    ] {
        let answer = send_as(&ses, &key, send, ALLOWED, &[RECIPIENT], None).await;
        let operation = send.operation();
        if send == Sending::Email {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (400, MESSAGE_REJECTED),
                "case {operation}: the action the policy allows must pass authorization, \
                 got {} {} {:?}",
                answer.status,
                answer.code,
                answer.message
            );
            continue;
        }
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (403, ACCESS_DENIED),
            "case {operation}: expected 403 {ACCESS_DENIED}, got {} {} {:?}",
            answer.status,
            answer.code,
            answer.message
        );
        assert_eq!(
            answer.message,
            not_authorized(&principal, operation, ALLOWED),
            "case {operation}: expected the refusal to name its own action, got {:?}",
            answer.message
        );
    }
}

/// One row of the document table: the policy in force, the send it is put to, and whether
/// authorization admits it.
struct Row {
    case: &'static str,
    document: String,
    source: &'static str,
    recipients: &'static [&'static str],
    return_path: Option<&'static str>,
    authorized: bool,
}

#[tokio::test]
async fn the_document_in_force_decides_the_send() {
    let other_region_arn = identity_arn("eu-west-1", ALLOWED);
    let rows = vec![
        Row {
            case: "no policy at all",
            document: String::new(),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "an unconditional allow of every sending action",
            document: allow_sending(None),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: true,
        },
        Row {
            case: "an allow of another service's actions only",
            document: policy(r#"{"Effect":"Allow","Action":"sns:Publish","Resource":"*"}"#),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "an explicit deny beside the allow",
            document: policy(
                r#"{"Effect":"Allow","Action":"ses:*","Resource":"*"},
                   {"Effect":"Deny","Action":"ses:SendEmail","Resource":"*"}"#,
            ),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "an allow scoped to the identity ARN the send is from",
            document: policy(&format!(
                r#"{{"Effect":"Allow","Action":"ses:SendEmail","Resource":"{}"}}"#,
                identity_arn(REGION, ALLOWED)
            )),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: true,
        },
        Row {
            case: "an allow scoped to another identity's ARN",
            document: policy(&format!(
                r#"{{"Effect":"Allow","Action":"ses:SendEmail","Resource":"{}"}}"#,
                identity_arn(REGION, DENIED)
            )),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "an allow scoped to the same identity in another Region",
            document: policy(&format!(
                r#"{{"Effect":"Allow","Action":"ses:SendEmail","Resource":"{other_region_arn}"}}"#
            )),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "a display-name condition the From value satisfies",
            document: allow_sending(Some(&string_equals("ses:FromDisplayName", &["Sender"]))),
            source: "\"Sender\" <allowed@authz.example.com>",
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: true,
        },
        Row {
            case: "a display-name condition a bare From address cannot satisfy",
            document: allow_sending(Some(&string_equals("ses:FromDisplayName", &["Sender"]))),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "a recipient condition every recipient satisfies",
            document: allow_sending(Some(&format!(
                r#"{{"ForAllValues:StringEquals":{{"ses:Recipients":["{RECIPIENT}"]}}}}"#
            ))),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: None,
            authorized: true,
        },
        Row {
            case: "a recipient condition one recipient of the send does not satisfy",
            document: allow_sending(Some(&format!(
                r#"{{"ForAllValues:StringEquals":{{"ses:Recipients":["{RECIPIENT}"]}}}}"#
            ))),
            source: ALLOWED,
            recipients: &[RECIPIENT, OTHER_RECIPIENT],
            return_path: None,
            authorized: false,
        },
        Row {
            case: "a feedback-address condition the ReturnPath satisfies",
            document: allow_sending(Some(&string_equals(
                "ses:FeedbackAddress",
                &["bounces@authz.example.com"],
            ))),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: Some("bounces@authz.example.com"),
            authorized: true,
        },
        Row {
            case: "a feedback-address condition and a send that names another one",
            document: allow_sending(Some(&string_equals(
                "ses:FeedbackAddress",
                &["bounces@authz.example.com"],
            ))),
            source: ALLOWED,
            recipients: &[RECIPIENT],
            return_path: Some("elsewhere@authz.example.com"),
            authorized: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One user per row, so no row is decided on another row's document.
        let ses = Emulator::new();
        let document = (!row.document.is_empty()).then(|| row.document.clone());
        let (key, principal) = ses
            .user(&format!("row-{index:02}"), document.as_deref())
            .await;
        let answer = send_as(
            &ses,
            &key,
            Sending::Email,
            row.source,
            row.recipients,
            row.return_path,
        )
        .await;
        if row.authorized {
            assert_eq!(
                (answer.status, answer.code.as_str()),
                (400, MESSAGE_REJECTED),
                "case {:?}: expected authorization to pass and the sender gate to answer, \
                 got {} {} {:?}",
                row.case,
                answer.status,
                answer.code,
                answer.message
            );
            continue;
        }
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (403, ACCESS_DENIED),
            "case {:?}: expected 403 {ACCESS_DENIED}, got {} {} {:?}",
            row.case,
            answer.status,
            answer.code,
            answer.message
        );
        assert_eq!(
            answer.message,
            not_authorized(
                &principal,
                "SendEmail",
                service_com_amazonaws_ses_simpleemailservice::state::addr_spec(row.source)
            ),
            "case {:?}: expected the authorization sentence, got {:?}",
            row.case,
            answer.message
        );
    }
}

#[tokio::test]
async fn the_accounts_own_credential_is_not_bound_by_any_identity_policy() {
    let ses = Emulator::new();
    // A user whose policy forbids every send exists on the account; the account's OWN
    // credential is a different caller and no identity policy governs it.
    ses.user(
        "denied-sender",
        Some(&policy(
            r#"{"Effect":"Deny","Action":"ses:*","Resource":"*"}"#,
        )),
    )
    .await;
    let answer = send_as(&ses, ACCOUNT_KEY, Sending::Email, DENIED, &[RECIPIENT], None).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, MESSAGE_REJECTED),
        "case the account's own credential: expected the sender gate to answer, got {} {} {:?}",
        answer.status,
        answer.code,
        answer.message
    );
}

#[tokio::test]
async fn an_authorized_send_from_a_verified_identity_is_accepted_and_logged() {
    let ses = Emulator::new();
    let (key, _) = ses
        .user(
            "verified-sender",
            Some(&allow_sending(Some(&string_equals(
                "ses:FromAddress",
                &[ALLOWED],
            )))),
        )
        .await;
    ses.verify(ALLOWED).await;
    ses.verify(DENIED).await;

    // The address the condition names: authorization passes, the identity is verified, and
    // the send is accepted and logged.
    let (status, body) = ses
        .ses(
            &key,
            "SendEmail",
            &Sending::Email.fields(ALLOWED, &[RECIPIENT], None),
        )
        .await;
    assert_eq!(
        status, 200,
        "case an authorized send from a verified identity: expected 200, got {status} {body}"
    );
    let message_id = element(&body, "MessageId")
        .map(|text| xml_text(&text))
        .unwrap_or_else(|| panic!("an accepted send answers a MessageId, got {body}"));
    assert!(
        !message_id.is_empty(),
        "case an accepted send: expected a stored MessageId, got {message_id:?}"
    );
    assert_eq!(
        ses.sent_last_24_hours().await,
        1.0,
        "case an accepted send: the send log must count its one recipient"
    );

    // The OTHER address is verified too, so only the policy stands between the caller and
    // an accepted send — and it refuses, leaving the log where it was.
    let answer = send_as(&ses, &key, Sending::Email, DENIED, &[RECIPIENT], None).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (403, ACCESS_DENIED),
        "case a verified identity the condition does not name: expected 403 {ACCESS_DENIED}, \
         got {} {} {:?}",
        answer.status,
        answer.code,
        answer.message
    );
    assert_eq!(
        ses.sent_last_24_hours().await,
        1.0,
        "case an unauthorized send: it must log nothing, and the earlier send must stand"
    );
}
