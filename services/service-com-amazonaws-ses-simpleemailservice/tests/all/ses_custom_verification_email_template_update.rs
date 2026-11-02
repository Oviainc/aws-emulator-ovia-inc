// SPDX-License-Identifier: CC0-1.0

//! `UpdateCustomVerificationEmailTemplate`, driven end to end over the awsQuery wire:
//! form-urlencoded body in, XML out, one shared engine behind every call, with the
//! round trip proved through `GetCustomVerificationEmailTemplate`.
//!
//! What is asserted is the RULE the operation follows over a SET of inputs, not one
//! recorded request:
//!
//! * an update replaces exactly the members its request CARRIES and leaves every
//!   member it omits at the value already stored — the name is the record's key, so it
//!   is the one member an update cannot change, and the template count is unchanged;
//! * an update names a template that must already exist: a name the account holds no
//!   template under is refused `CustomVerificationEmailTemplateDoesNotExist` and
//!   creates nothing;
//! * content at or past the size the model documents for a custom verification email
//!   is refused `CustomVerificationEmailInvalidContent`, and content inside it is
//!   stored;
//! * the sender the template is LEFT with must be a verified sending identity of this
//!   account — the address the request names when it names one, the address the
//!   template keeps when it does not — and anything else is refused
//!   `FromEmailAddressNotVerified`;
//! * every refusal leaves the stored template exactly as it stood.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications, xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The five members of a stored custom verification email template that an update can
/// replace. The name is the record's key and is carried separately.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Template {
    from: String,
    subject: String,
    content: String,
    success_url: String,
    failure_url: String,
}

impl Template {
    /// A template sent from `from`, with every other member at a plain value a row
    /// varies from.
    fn new(from: &str) -> Self {
        Self {
            from: from.to_string(),
            subject: "Please verify your address".to_string(),
            content: "<html><body>Please verify your address.</body></html>".to_string(),
            success_url: "https://example.com/verified".to_string(),
            failure_url: "https://example.com/failed".to_string(),
        }
    }

    /// The form fields of a `CreateCustomVerificationEmailTemplate` for it, in the
    /// order the model lists them.
    fn create_params(&self, name: &str) -> Vec<Field> {
        vec![
            field("TemplateName", name),
            field("FromEmailAddress", self.from.clone()),
            field("TemplateSubject", self.subject.clone()),
            field("TemplateContent", self.content.clone()),
            field("SuccessRedirectionURL", self.success_url.clone()),
            field("FailureRedirectionURL", self.failure_url.clone()),
        ]
    }
}

/// The members an update CARRIES: every one of them is optional on the wire, so `None`
/// is a member the request leaves out altogether.
#[derive(Debug, Clone, Default)]
struct Update {
    from: Option<String>,
    subject: Option<String>,
    content: Option<String>,
    success_url: Option<String>,
    failure_url: Option<String>,
}

impl Update {
    /// The form fields of an `UpdateCustomVerificationEmailTemplate` for `name`: only
    /// the members this update carries appear on the wire.
    fn params(&self, name: &str) -> Vec<Field> {
        let mut params = vec![field("TemplateName", name)];
        for (member, value) in [
            ("FromEmailAddress", self.from.as_ref()),
            ("TemplateSubject", self.subject.as_ref()),
            ("TemplateContent", self.content.as_ref()),
            ("SuccessRedirectionURL", self.success_url.as_ref()),
            ("FailureRedirectionURL", self.failure_url.as_ref()),
        ] {
            if let Some(value) = value {
                params.push(field(member, value.clone()));
            }
        }
        params
    }

    /// The record this update leaves behind `stored`: the members it carries replace
    /// the stored ones, the members it omits keep them.
    fn applied_to(&self, stored: &Template) -> Template {
        Template {
            from: self.from.clone().unwrap_or_else(|| stored.from.clone()),
            subject: self.subject.clone().unwrap_or_else(|| stored.subject.clone()),
            content: self.content.clone().unwrap_or_else(|| stored.content.clone()),
            success_url: self
                .success_url
                .clone()
                .unwrap_or_else(|| stored.success_url.clone()),
            failure_url: self
                .failure_url
                .clone()
                .unwrap_or_else(|| stored.failure_url.clone()),
        }
    }

    /// An update carrying nothing but the name.
    fn nothing() -> Self {
        Self::default()
    }

    /// An update carrying only `TemplateSubject`.
    fn subject(subject: &str) -> Self {
        Self {
            subject: Some(subject.to_string()),
            ..Self::default()
        }
    }

    /// An update carrying only `FromEmailAddress`.
    fn from(from: &str) -> Self {
        Self {
            from: Some(from.to_string()),
            ..Self::default()
        }
    }

    /// An update carrying only `TemplateContent`.
    fn content(content: &str) -> Self {
        Self {
            content: Some(content.to_string()),
            ..Self::default()
        }
    }
}

/// `CreateCustomVerificationEmailTemplate`, asserted to land.
async fn seed(router: &MultiServiceRouter, name: &str, template: &Template) {
    let (status, body) = call(
        router,
        "CreateCustomVerificationEmailTemplate",
        &template.create_params(name),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seeding {name:?}: expected 200, got {status} with {body}"
    );
}

/// `UpdateCustomVerificationEmailTemplate`.
async fn update(router: &MultiServiceRouter, name: &str, update: &Update) -> (u16, String) {
    call(
        router,
        "UpdateCustomVerificationEmailTemplate",
        &update.params(name),
    )
    .await
}

/// `GetCustomVerificationEmailTemplate`.
async fn get(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "GetCustomVerificationEmailTemplate",
        &[field("TemplateName", name)],
    )
    .await
}

/// One member of a `GetCustomVerificationEmailTemplate` response.
fn reported(body: &str, member: &str) -> String {
    element(body, member)
        .map(|raw| xml_text(&raw))
        .unwrap_or_default()
}

/// The head of a body, for a failure message that must not carry megabytes of it.
///
/// Taken in CHARACTERS, so a cut never lands inside a multi-byte one.
fn head(body: &str) -> String {
    body.chars().take(512).collect()
}

/// Assert that `expected` is what the store holds under `name`, member by member.
async fn assert_stored(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &Template,
) {
    let (status, body) = get(router, name).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected 200 reading {name:?} back, got {status} with {}",
        head(&body)
    );
    for (member, want) in [
        ("TemplateName", name.to_string()),
        ("FromEmailAddress", expected.from.clone()),
        ("TemplateSubject", expected.subject.clone()),
        ("SuccessRedirectionURL", expected.success_url.clone()),
        ("FailureRedirectionURL", expected.failure_url.clone()),
    ] {
        let got = reported(&body, member);
        assert_eq!(
            got, want,
            "case {case:?}: expected {member} {want:?}, got {got:?}"
        );
    }
    // The content is compared whole, but a mismatch is reported by LENGTH: a row may
    // carry megabytes of it and a failure message must stay readable.
    let got = reported(&body, "TemplateContent");
    assert!(
        got == expected.content,
        "case {case:?}: expected TemplateContent of {} bytes, got {} bytes starting {:?}",
        expected.content.len(),
        got.len(),
        head(&got)
    );
}

/// Assert that nothing is stored under `name`: the read reports the modelled absence.
async fn assert_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = get(router, name).await;
    assert_eq!(
        (status, error_envelope(&body)),
        (
            400,
            (
                Some("CustomVerificationEmailTemplateDoesNotExist".to_string()),
                // Entirely fixed text: the refusal does not quote the name asked for.
                Some("Provided custom verification email template name does not exist.".to_string())
            )
        ),
        "case {case:?}: expected the modelled absence for {name:?}, got {status} with {body}"
    );
}

/// `VerifyEmailIdentity`, carried through to a SENDING identity.
///
/// Asking for verification only starts it: the move to `Success` the clock owes the
/// record falls due a span later. This suite is about the update operation and not
/// about that span, so it settles the store past it (the harness's
/// [`settle_verifications`]) rather than sleeping for it.
async fn verify_email_identity(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    address: &str,
) {
    let (status, body) = call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case verify {address:?}: expected 200, got {status} with {body}"
    );
    settle_verifications(engine);
}

/// `VerifyDomainIdentity`, carried through the same way.
async fn verify_domain_identity(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    domain: &str,
) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "case verify {domain:?}: expected 200, got {status} with {body}"
    );
    settle_verifications(engine);
}

/// `DeleteIdentity`, so an address this account had verified stops being a sender.
async fn delete_identity(router: &MultiServiceRouter, identity: &str) {
    let (status, body) = call(router, "DeleteIdentity", &[field("Identity", identity)]).await;
    assert_eq!(
        status, 200,
        "case delete identity {identity:?}: expected 200, got {status} with {body}"
    );
}

/// The refusal an unverified sender is answered with, as `(code, message)`.
fn unverified(address: &str) -> (Option<String>, Option<String>) {
    (
        Some("FromEmailAddressNotVerified".to_string()),
        Some(format!("The from email address {address} is not verified.")),
    )
}

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_keeps_the_rest() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;
    verify_email_identity(&router, &engine, "second@example.com").await;
    verify_domain_identity(&router, &engine, "example.net").await;

    const NAME: &str = "verify-partial-updates";
    let mut expected = Template::new("verification@example.com");
    seed(&router, NAME, &expected).await;

    // The rows differ along what the rule generalises over: WHICH subset of the five
    // replaceable members the request carries (none, each one alone, a pair, all
    // five), and the FORM of the values in them (plain text, markup and an ampersand,
    // non-ASCII text, a one-character body, URLs carrying a path and a query, a
    // mailbox identity and a mailbox a verified domain covers). They are applied in
    // sequence to ONE template, so each row also asserts that the members the previous
    // rows wrote survive an update that does not mention them.
    let rows: Vec<(&str, Update)> = vec![
        ("nothing but the name", Update::nothing()),
        ("only the subject", Update::subject("Confirm your address")),
        (
            "only the content",
            Update::content("<html><body>Confirm, please.</body></html>"),
        ),
        ("only the sender", Update::from("second@example.com")),
        (
            "only the success URL",
            Update {
                success_url: Some("https://example.com/ses/ok?token=1&next=/home".to_string()),
                ..Update::default()
            },
        ),
        (
            "only the failure URL",
            Update {
                failure_url: Some("http://example.com/ses/no?token=1&next=/help".to_string()),
                ..Update::default()
            },
        ),
        (
            "the subject and the content together",
            Update {
                subject: Some("Confirm & continue <now>".to_string()),
                content: Some("<html><body><p>Hi &amp; welcome</p></body></html>".to_string()),
                ..Update::default()
            },
        ),
        (
            "non-ASCII subject and content",
            Update {
                subject: Some("Bitte bestätigen Sie Ihre Adresse".to_string()),
                content: Some("<html><body>Adresse bestätigen — danke!</body></html>".to_string()),
                ..Update::default()
            },
        ),
        (
            "content of one character, the shortest a template can carry",
            Update::content("x"),
        ),
        (
            "a sender a verified domain covers, under a subdomain of it",
            Update::from("noreply@mail.example.net"),
        ),
        (
            "all five members at once",
            Update {
                from: Some("noreply@example.net".to_string()),
                subject: Some("Final subject".to_string()),
                content: Some("<html><body>Final body.</body></html>".to_string()),
                success_url: Some("https://example.net/ok".to_string()),
                failure_url: Some("https://example.net/no".to_string()),
            },
        ),
        (
            "the identical call again, which changes nothing",
            Update {
                from: Some("noreply@example.net".to_string()),
                subject: Some("Final subject".to_string()),
                content: Some("<html><body>Final body.</body></html>".to_string()),
                success_url: Some("https://example.net/ok".to_string()),
                failure_url: Some("https://example.net/no".to_string()),
            },
        ),
    ];
    for (case, carried) in &rows {
        expected = carried.applied_to(&expected);
        let (status, body) = update(&router, NAME, carried).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_stored(&router, case, NAME, &expected).await;
    }
}

#[tokio::test]
async fn an_update_names_a_template_that_must_already_exist() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;

    const STORED: &str = "verify-stored";
    let held = Template::new("verification@example.com");
    seed(&router, STORED, &held).await;

    // The rows differ in the FORM of the name that misses, measured against a name the
    // store DOES hold: never created, another case of the stored one, a prefix of it,
    // the stored one with trailing whitespace, the empty name, a name carrying XML
    // metacharacters, a non-ASCII name, and a long one.
    let names: Vec<(&str, String)> = vec![
        ("a name never created", "no-such-template".to_string()),
        ("another case of the stored name", STORED.to_uppercase()),
        ("a prefix of the stored name", "verify".to_string()),
        (
            "the stored name with trailing whitespace",
            format!("{STORED} "),
        ),
        ("the empty name", String::new()),
        (
            "a name carrying XML metacharacters",
            "tmpl<&>\"'absent".to_string(),
        ),
        ("a non-ASCII name", "vorlage-bestätigung".to_string()),
        ("a name of 512 characters", "absent-".repeat(64) + "x"),
    ];
    // The update carried alongside each name varies too, so the refusal is about the
    // name and not about what the request would have written.
    let carried: Vec<Update> = vec![
        Update::nothing(),
        Update::subject("Something else"),
        Update {
            from: Some("verification@example.com".to_string()),
            subject: Some("Something else".to_string()),
            content: Some("<html><body>Else.</body></html>".to_string()),
            success_url: Some("https://example.com/other-ok".to_string()),
            failure_url: Some("https://example.com/other-no".to_string()),
        },
    ];
    for (index, (case, name)) in names.iter().enumerate() {
        let carried = &carried[index % carried.len()];
        let (status, body) = update(&router, name, carried).await;
        assert_eq!(
            (status, error_envelope(&body)),
            (
                400,
                (
                    Some("CustomVerificationEmailTemplateDoesNotExist".to_string()),
                    Some(
                        "Provided custom verification email template name does not exist."
                            .to_string()
                    )
                )
            ),
            "case {case:?}: expected the modelled absence for {name:?}, got {status} with {body}"
        );
        // An update never creates: the name is still free afterwards.
        assert_absent(&router, case, name).await;
        // And the template the store does hold is untouched by the refusal.
        assert_stored(&router, case, STORED, &held).await;
    }
}

#[tokio::test]
async fn content_the_email_cannot_carry_is_refused_and_the_stored_template_stands() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;

    const NAME: &str = "verify-content-bound";
    let mut expected = Template::new("verification@example.com");
    seed(&router, NAME, &expected).await;

    // The documented bound is on the size of the email, so content ON it and past it
    // is content no custom verification email can be built from, and content inside it
    // is stored. The rows are the bound itself and one byte either side of it.
    let inside = Update::content(&"c".repeat(MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES - 1));
    let (status, body) = update(&router, NAME, &inside).await;
    assert_eq!(
        status,
        200,
        "case content one byte inside the bound: expected 200, got {status} with {}",
        head(&body)
    );
    expected = inside.applied_to(&expected);
    assert_stored(
        &router,
        "content one byte inside the bound",
        NAME,
        &expected,
    )
    .await;

    for (case, size) in [
        ("content on the bound", MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES),
        (
            "content one byte past the bound",
            MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES + 1,
        ),
    ] {
        let over = Update::content(&"d".repeat(size));
        let (status, body) = update(&router, NAME, &over).await;
        assert_eq!(
            status,
            400,
            "case {case:?}: expected 400, got {status} with {}",
            head(&body)
        );
        assert_eq!(
            error_envelope(&body).0,
            Some("CustomVerificationEmailInvalidContent".to_string()),
            "case {case:?}: expected the invalid-content refusal, got {}",
            head(&body)
        );
        // Refused before anything was written: the stored content still stands.
        assert_stored(&router, case, NAME, &expected).await;
    }
}

#[tokio::test]
async fn the_sender_the_update_leaves_must_be_a_verified_identity() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;
    verify_domain_identity(&router, &engine, "mail.example.net").await;

    const NAME: &str = "verify-sender-gate";
    let held = Template::new("verification@example.com");
    seed(&router, NAME, &held).await;

    // The rows differ in HOW the sender the update names fails the identity check: an
    // address at a domain this account holds no identity for, an address at a domain
    // that merely resembles a verified one, the PARENT of a verified domain (a verified
    // domain covers what is under it, never what is above it), and another mailbox at
    // the unverified domain of a verified mailbox.
    let rows: &[(&str, &str)] = &[
        ("sender at a domain with no identity", "sender@example.org"),
        (
            "sender at a domain that only resembles a verified one",
            "sender@notmail.example.net",
        ),
        ("sender at the parent of a verified domain", "sender@example.net"),
        (
            "another mailbox at the unverified domain of a verified mailbox",
            "someone-else@example.com",
        ),
    ];
    for (case, sender) in rows {
        // The update carries the subject as well, so a refusal is shown to drop the
        // WHOLE update and not only the sender.
        let carried = Update {
            from: Some(sender.to_string()),
            subject: Some("A subject the refusal must not store".to_string()),
            ..Update::default()
        };
        let (status, body) = update(&router, NAME, &carried).await;
        assert_eq!(
            (status, error_envelope(&body)),
            (400, unverified(sender)),
            "case {case:?}: expected the unverified-sender refusal, got {status} with {body}"
        );
        assert_stored(&router, case, NAME, &held).await;

        // The positive control: the SAME call once that address is a verified identity
        // of this account. Only the identity's presence differs, and now it lands and
        // reads back.
        verify_email_identity(&router, &engine, sender).await;
        let (status, body) = update(&router, NAME, &carried).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once the sender is verified, got {status} with {body}"
        );
        assert_stored(&router, case, NAME, &carried.applied_to(&held)).await;

        // Back to the template this suite's rows start from, for the next row.
        let restore = Update {
            from: Some(held.from.clone()),
            subject: Some(held.subject.clone()),
            ..Update::default()
        };
        let (status, body) = update(&router, NAME, &restore).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 restoring the template, got {status} with {body}"
        );
        assert_stored(&router, case, NAME, &held).await;
    }
}

#[tokio::test]
async fn an_update_that_omits_the_sender_is_judged_on_the_sender_the_template_keeps() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;

    const NAME: &str = "verify-kept-sender";
    let held = Template::new("verification@example.com");
    seed(&router, NAME, &held).await;

    // While the stored sender is a verified identity, an update that says nothing about
    // it goes through: the address gated is the one the template is LEFT with.
    let subject_only = Update::subject("Confirm your address");
    let (status, body) = update(&router, NAME, &subject_only).await;
    assert_eq!(
        status, 200,
        "case a verified stored sender: expected 200, got {status} with {body}"
    );
    let expected = subject_only.applied_to(&held);
    assert_stored(&router, "a verified stored sender", NAME, &expected).await;

    // The same call once that identity is gone is refused on the address the template
    // keeps, and stores nothing — the rows differ in whether the update mentions the
    // sender at all.
    delete_identity(&router, &held.from).await;
    for (case, carried) in [
        ("nothing but the name", Update::nothing()),
        ("only the subject", Update::subject("Another subject")),
        (
            "only the content",
            Update::content("<html><body>Another body.</body></html>"),
        ),
    ] {
        let (status, body) = update(&router, NAME, &carried).await;
        assert_eq!(
            (status, error_envelope(&body)),
            (400, unverified(&held.from)),
            "case {case:?} with the stored sender no longer verified: expected the \
             unverified-sender refusal, got {status} with {body}"
        );
        assert_stored(&router, case, NAME, &expected).await;
    }

    // And once that address is a verified identity again, the same update lands.
    verify_email_identity(&router, &engine, &held.from).await;
    let carried = Update::subject("Another subject");
    let (status, body) = update(&router, NAME, &carried).await;
    assert_eq!(
        status, 200,
        "case the stored sender verified again: expected 200, got {status} with {body}"
    );
    assert_stored(
        &router,
        "the stored sender verified again",
        NAME,
        &carried.applied_to(&expected),
    )
    .await;
}
