// SPDX-License-Identifier: CC0-1.0

//! What `CreateCustomVerificationEmailTemplate` stores, and how the read of one
//! template answers a name the account holds nothing under, end to end over the
//! wire.
//!
//! The suite beside this one (`ses_custom_verification_email_templates`) asserts
//! the rules the create itself follows — the sender gate, the naming conflict,
//! the content ceiling, the account limit. What is asserted HERE is the two
//! rules those rows do not reach, each over a SET of inputs rather than one
//! request:
//!
//! * every member a create carried is stored VERBATIM and reported back
//!   unchanged, whatever text it is — plain, HTML, XML metacharacters, non-ASCII,
//!   empty, URLs carrying query strings, a sender written in display-name form —
//!   and a later create of another name leaves it alone, so the store keeps one
//!   record per name;
//! * a template name is a KEY, compared exactly: a read of a name that misses —
//!   never created, another case of a stored one, a prefix of it, the empty name
//!   — is refused with the modelled
//!   `CustomVerificationEmailTemplateDoesNotExist` and its wording for that
//!   name.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind
//! all of them.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications, xml_text,
    Field,
};
use smithy_server_runtime::MultiServiceRouter;

/// One custom verification email template as a request carries it. Every member
/// is required by the model, so a row names them all.
#[derive(Debug, Clone)]
struct TemplateRow {
    case: &'static str,
    name: String,
    from: String,
    subject: String,
    content: String,
    success_url: String,
    failure_url: String,
}

/// What a reader reports for one template.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedTemplate {
    name: Option<String>,
    from: Option<String>,
    subject: Option<String>,
    content: Option<String>,
    success_url: Option<String>,
    failure_url: Option<String>,
}

impl TemplateRow {
    /// A row naming every part.
    fn new(
        case: &'static str,
        name: &str,
        from: &str,
        subject: &str,
        content: &str,
        success_url: &str,
        failure_url: &str,
    ) -> Self {
        Self {
            case,
            name: name.to_string(),
            from: from.to_string(),
            subject: subject.to_string(),
            content: content.to_string(),
            success_url: success_url.to_string(),
            failure_url: failure_url.to_string(),
        }
    }

    /// The form fields one `CreateCustomVerificationEmailTemplate` call carries.
    fn fields(&self) -> Vec<Field> {
        vec![
            field("TemplateName", self.name.clone()),
            field("FromEmailAddress", self.from.clone()),
            field("TemplateSubject", self.subject.clone()),
            field("TemplateContent", self.content.clone()),
            field("SuccessRedirectionURL", self.success_url.clone()),
            field("FailureRedirectionURL", self.failure_url.clone()),
        ]
    }

    /// What the reader must report once this row is stored: each member as the
    /// request wrote it.
    fn as_reported(&self) -> ReportedTemplate {
        ReportedTemplate {
            name: Some(self.name.clone()),
            from: Some(self.from.clone()),
            subject: Some(self.subject.clone()),
            content: Some(self.content.clone()),
            success_url: Some(self.success_url.clone()),
            failure_url: Some(self.failure_url.clone()),
        }
    }
}

/// `CreateCustomVerificationEmailTemplate`, asserting it was accepted.
async fn create(router: &MultiServiceRouter, row: &TemplateRow) {
    let (status, body) = call(
        router,
        "CreateCustomVerificationEmailTemplate",
        &row.fields(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {}: expected 200 from CreateCustomVerificationEmailTemplate, got {status} with \
         {body}",
        row.case
    );
    assert!(
        body.contains("<CreateCustomVerificationEmailTemplateResponse"),
        "case {}: expected this operation's awsQuery envelope, got {body}",
        row.case
    );
}

/// One `GetCustomVerificationEmailTemplate` call: `(status, body)`.
async fn get_raw(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "GetCustomVerificationEmailTemplate",
        &[field("TemplateName", name)],
    )
    .await
}

/// `GetCustomVerificationEmailTemplate`, as the parts it reports.
async fn get(router: &MultiServiceRouter, case: &str, name: &str) -> ReportedTemplate {
    let (status, body) = get_raw(router, name).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GetCustomVerificationEmailTemplate({name:?}), got \
         {status} with {body}"
    );
    let part = |tag: &str| element(&body, tag).map(|text| xml_text(&text));
    ReportedTemplate {
        name: part("TemplateName"),
        from: part("FromEmailAddress"),
        subject: part("TemplateSubject"),
        content: part("TemplateContent"),
        success_url: part("SuccessRedirectionURL"),
        failure_url: part("FailureRedirectionURL"),
    }
}

/// `VerifyEmailIdentity`, asserting it was accepted.
async fn verify_email(router: &MultiServiceRouter, address: &str) {
    let (status, body) = call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the verified mailbox {address:?}: expected 200, got {status} with {body}"
    );
}

/// `VerifyDomainIdentity`, asserting it was accepted.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "seeding the verified domain {domain:?}: expected 200, got {status} with {body}"
    );
}

/// The mailbox the rows send from.
const SENDER: &str = "verification@example.com";

/// A domain this account verifies, which covers the mailboxes under it.
const SENDER_DOMAIN: &str = "example.net";

/// A router whose account already verifies [`SENDER`] and [`SENDER_DOMAIN`].
///
/// Asking for an identity's verification only STARTS it, so the store is settled
/// against an instant past the span the clock owes that move
/// (`harness::settle_verifications`) instead of sleeping for it: these rows are
/// about what a create stores, not about that span.
async fn emulator_with_verified_senders() -> MultiServiceRouter {
    let (router, engine) = emulator_and_engine();
    verify_email(&router, SENDER).await;
    verify_domain(&router, SENDER_DOMAIN).await;
    settle_verifications(&engine);
    router
}

#[tokio::test]
async fn every_member_a_create_carried_is_stored_verbatim_and_reported_back() {
    let router = emulator_with_verified_senders().await;

    // The rows differ along what the rule generalises over: the form of the
    // name, which verified sender the template is sent from and in which form,
    // and the kind of text each part carries — plain, HTML, XML
    // metacharacters, non-ASCII, empty, and URLs with and without query
    // strings. None of them is rewritten on the way in or out.
    let rows = [
        TemplateRow::new(
            "plain text content",
            "confirm-your-address",
            SENDER,
            "Please confirm your address",
            "Follow the link to confirm your address.",
            "https://example.com/verified",
            "https://example.com/failed",
        ),
        TemplateRow::new(
            "HTML content, sender under a verified domain",
            "confirm.html.v2",
            "noreply@mail.example.net",
            "Confirm your email",
            "<html><body><p>Confirm <b>now</b></p></body></html>",
            "https://example.net/ok?src=ses&lang=en",
            "https://example.net/no?src=ses&lang=en",
        ),
        TemplateRow::new(
            "content carrying XML metacharacters",
            "metacharacters",
            SENDER,
            "Subject with <angles> & \"quotes\"",
            "5 > 3 && 2 < 4 — 'confirm' now",
            "https://example.com/ok?a=1&b=2",
            "https://example.com/no?a=1&b=2",
        ),
        TemplateRow::new(
            "non-ASCII subject and content",
            "bestätigung",
            SENDER,
            "Bitte bestätigen Sie Ihre Adresse — 確認",
            "Klicken Sie auf den Link. 🙂",
            "https://example.com/bestätigt",
            "https://example.com/fehlgeschlagen",
        ),
        TemplateRow::new(
            "empty subject and content, which the model admits",
            "empty-parts",
            "ops@sub.mail.example.net",
            "",
            "",
            "https://example.net/ok",
            "https://example.net/no",
        ),
        TemplateRow::new(
            "sender in display-name form",
            "display-name-sender",
            "Verification <verification@example.com>",
            "Confirm",
            "Confirm your address.",
            "https://example.com/ok",
            "https://example.com/no",
        ),
    ];

    for row in &rows {
        create(&router, row).await;
        let reported = get(&router, row.case, &row.name).await;
        assert_eq!(
            reported,
            row.as_reported(),
            "case {}: expected the reader to report the parts that were sent, got {reported:?}",
            row.case
        );
    }

    // Each template is still reported after the later creates, so one create
    // never rewrote or displaced another's record.
    for row in &rows {
        let reported = get(&router, row.case, &row.name).await;
        assert_eq!(
            reported,
            row.as_reported(),
            "case {}: expected the stored template to survive the later creates, got {reported:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_template_name_is_a_key_compared_exactly() {
    let router = emulator_with_verified_senders().await;

    // Two names differing only in case are two different keys, so both creates
    // are accepted and each name reports its own template.
    let lower = TemplateRow::new(
        "lower-case name",
        "confirm-your-address",
        SENDER,
        "Please confirm",
        "The lower-case template.",
        "https://example.com/ok",
        "https://example.com/no",
    );
    let upper = TemplateRow::new(
        "same name in another case",
        "Confirm-Your-Address",
        SENDER,
        "Please confirm",
        "The mixed-case template.",
        "https://example.com/ok",
        "https://example.com/no",
    );
    create(&router, &lower).await;
    create(&router, &upper).await;
    for row in [&lower, &upper] {
        let reported = get(&router, row.case, &row.name).await;
        assert_eq!(
            reported,
            row.as_reported(),
            "case {}: expected each case of the name to report its own template, got {reported:?}",
            row.case
        );
    }

    // The rows differ in how a name MISSES: never created, another case of a
    // stored one, a prefix of it, a stored name with trailing space, and the
    // empty name. Each is refused with the modelled absence, worded for the name
    // the request carried.
    for (case, name) in [
        ("a name never created", "no-such-template"),
        ("another case of a stored name", "CONFIRM-YOUR-ADDRESS"),
        ("a prefix of a stored name", "confirm-your"),
        ("a stored name with a trailing space", "confirm-your-address "),
        ("the empty name", ""),
    ] {
        let (status, body) = get_raw(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 reading {name:?}, got {status} with {body}"
        );
        assert_eq!(
            error_envelope(&body),
            (
                Some("CustomVerificationEmailTemplateDoesNotExist".to_string()),
                // The wording AWS states for this error class, as a literal here:
                // only the name is taken from the request.
                Some(format!(
                    "Custom verification email template <{name}> does not exist."
                ))
            ),
            "case {case:?}: expected the modelled absence for {name:?}, got {body}"
        );
    }
}
