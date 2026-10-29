// SPDX-License-Identifier: CC0-1.0

//! `CreateCustomVerificationEmailTemplate` and the read that proves it, end to end
//! over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind all of
//! them. What is asserted is the RULE each operation follows over a SET of inputs, not
//! one recorded request:
//!
//! * a create whose `FromEmailAddress` is not a VERIFIED sending identity of this
//!   account is refused `FromEmailAddressNotVerified` and persists NOTHING — the
//!   following `GetCustomVerificationEmailTemplate` for that name reports the template
//!   does not exist — while the identical call differing only in that identity's
//!   verification succeeds and reads back. An identity whose verification was only
//!   REQUESTED is not eligible to send, so it is not a sender a template may be
//!   created for either;
//! * an accepted create stores every member the request carried, and
//!   `GetCustomVerificationEmailTemplate` reports each of them unchanged;
//! * creating is not replacing: a name the store already holds is refused
//!   `CustomVerificationEmailTemplateAlreadyExists` and the stored template stands;
//! * content past the size the model documents for a custom verification email is
//!   refused `CustomVerificationEmailInvalidContent`, and content inside it is stored;
//! * a create past the account's ceiling on these templates is refused
//!   `LimitExceeded` and stores nothing.

use crate::harness::{
    call, element, emulator_and_engine, emulator_over, field, settle_verifications, xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{
    self, IdentityType, VerificationStatus, MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES,
    MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The six members a create carries, in the order the model lists them.
#[derive(Debug, Clone)]
struct Template {
    name: String,
    from: String,
    subject: &'static str,
    content: String,
    success_url: &'static str,
    failure_url: &'static str,
}

impl Template {
    /// A template whose members differ only in the parts a row varies.
    fn new(name: &str, from: &str) -> Self {
        Self {
            name: name.to_string(),
            from: from.to_string(),
            subject: "Please verify your address",
            content: "<html><body>Please verify your address.</body></html>".to_string(),
            success_url: "https://example.com/verified",
            failure_url: "https://example.com/failed",
        }
    }

    /// The form fields of a `CreateCustomVerificationEmailTemplate` request for it.
    fn params(&self) -> Vec<Field> {
        vec![
            field("TemplateName", self.name.clone()),
            field("FromEmailAddress", self.from.clone()),
            field("TemplateSubject", self.subject),
            field("TemplateContent", self.content.clone()),
            field("SuccessRedirectionURL", self.success_url),
            field("FailureRedirectionURL", self.failure_url),
        ]
    }
}

/// `CreateCustomVerificationEmailTemplate`.
async fn create(router: &MultiServiceRouter, template: &Template) -> (u16, String) {
    call(
        router,
        "CreateCustomVerificationEmailTemplate",
        &template.params(),
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

/// `VerifyEmailIdentity`, carried through to a SENDING identity.
///
/// Asking for verification only starts it: the move to `Success` the clock owes the
/// record falls due a span later. These suites are about the template operations and
/// not about that span, so each one settles the store past it (the harness's
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

/// The `<Code>` of an awsQuery `<ErrorResponse>` envelope.
fn code(body: &str) -> String {
    element(body, "Code").unwrap_or_default()
}

/// The `<Message>` of an awsQuery `<ErrorResponse>` envelope, unescaped.
fn message(body: &str) -> String {
    element(body, "Message")
        .map(|raw| xml_text(&raw))
        .unwrap_or_default()
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

/// Assert that nothing is stored under `name`: the read reports the modelled absence.
async fn assert_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = get(router, name).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 reading {name:?} back, got {status} with {body}"
    );
    assert_eq!(
        (code(&body).as_str(), message(&body).as_str()),
        (
            "CustomVerificationEmailTemplateDoesNotExist",
            format!("Custom verification email template <{name}> does not exist.").as_str()
        ),
        "case {case:?}: expected the modelled absence for {name:?}, got {body}"
    );
}

/// Assert that `template` is what the store holds, member by member.
async fn assert_stored(router: &MultiServiceRouter, case: &str, template: &Template) {
    let (status, body) = get(router, &template.name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 reading {:?} back, got {status} with {body}",
        template.name
    );
    for (member, expected) in [
        ("TemplateName", template.name.clone()),
        ("FromEmailAddress", template.from.clone()),
        ("TemplateSubject", template.subject.to_string()),
        ("TemplateContent", template.content.clone()),
        ("SuccessRedirectionURL", template.success_url.to_string()),
        ("FailureRedirectionURL", template.failure_url.to_string()),
    ] {
        let got = reported(&body, member);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {member} {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_created_template_is_reported_member_for_member_by_the_read() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;
    verify_domain_identity(&router, &engine, "example.net").await;

    // The rows differ along what the rule generalises over: the sender (a mailbox
    // that is an identity of its own, and mailboxes a verified DOMAIN covers in it
    // and in a subdomain of it), the form of the name, the subject and content
    // (plain text, HTML, markup-bearing and non-ASCII text), and the shape of the
    // two URLs.
    let rows: Vec<(&str, Template)> = vec![
        (
            "sender that is a mailbox identity",
            Template::new("verify-plain", "verification@example.com"),
        ),
        (
            "sender covered by a verified domain",
            Template::new("verify-under-a-domain", "noreply@example.net"),
        ),
        (
            "sender under a subdomain of a verified domain",
            Template::new("verify-under-a-subdomain", "noreply@mail.example.net"),
        ),
        (
            "subject and content carrying markup and an ampersand",
            Template {
                subject: "Confirm & continue <now>",
                content: "<html><body><p>Hi &amp; welcome</p></body></html>".to_string(),
                ..Template::new("verify-with-markup", "verification@example.com")
            },
        ),
        (
            "non-ASCII subject and content",
            Template {
                subject: "Bitte bestätigen Sie Ihre Adresse",
                content: "<html><body>Adresse bestätigen — danke!</body></html>".to_string(),
                ..Template::new("verify-non-ascii", "verification@example.com")
            },
        ),
        (
            "URLs carrying a path and a query",
            Template {
                success_url: "http://example.com/ses/ok?token=1&next=/home",
                failure_url: "http://example.com/ses/no?token=1&next=/help",
                ..Template::new("verify-with-query-urls", "noreply@example.net")
            },
        ),
        (
            "content of one character, the shortest a template can carry",
            Template {
                content: "x".to_string(),
                ..Template::new("verify-one-character", "verification@example.com")
            },
        ),
    ];
    for (case, template) in &rows {
        let (status, body) = create(&router, template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_stored(&router, case, template).await;
    }
}

#[tokio::test]
async fn an_unverified_sender_is_refused_and_leaves_nothing_behind() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;
    verify_domain_identity(&router, &engine, "mail.example.net").await;

    // The rows differ in HOW the sender fails the identity check: an address of a
    // domain this account holds no identity for, an address at a domain that merely
    // looks like a verified one, the PARENT of a verified domain (a verified domain
    // covers what is under it, never what is above it), and an address whose mailbox
    // differs from a verified mailbox at an unverified domain.
    let rows: &[(&str, &str, &str)] = &[
        (
            "sender at a domain with no identity",
            "verify-unknown-domain",
            "sender@example.org",
        ),
        (
            "sender at a domain that only resembles a verified one",
            "verify-lookalike-domain",
            "sender@notmail.example.net",
        ),
        (
            "sender at the parent of a verified domain",
            "verify-parent-domain",
            "sender@example.net",
        ),
        (
            "another mailbox at the unverified domain of a verified mailbox",
            "verify-other-mailbox",
            "someone-else@example.com",
        ),
    ];
    for (case, name, sender) in rows {
        let template = Template::new(name, sender);
        let (status, body) = create(&router, &template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            (code(&body).as_str(), message(&body).as_str()),
            (
                "FromEmailAddressNotVerified",
                format!("The from email address {sender} is not verified.").as_str()
            ),
            "case {case:?}: expected the unverified-sender refusal, got {body}"
        );
        // Refused before anything was written: the name is still free.
        assert_absent(&router, case, name).await;

        // The positive control: the SAME call once the sender is an identity of this
        // account. Only the identity's presence differs, and now it lands and reads
        // back.
        verify_email_identity(&router, &engine, sender).await;
        let (status, body) = create(&router, &template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once the sender is verified, got {status} with {body}"
        );
        assert_stored(&router, case, &template).await;
    }
}

#[tokio::test]
async fn only_a_sender_whose_verification_succeeded_is_accepted() {
    // The identity list is seeded through this service's own store, because what this
    // rule reads is the identity's verification STATUS, and the v1 verify operations
    // record only one of that enum's values today.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    // One row per value of the stored status enum (`state::VerificationStatus`), so the
    // rule is asserted over the whole set rather than over the one state a verify call
    // happens to write. `accepted` is whether that state makes the address eligible to
    // send the custom verification email, which is what the refusal reports.
    let seeded: &[(&str, &str, VerificationStatus, bool)] = &[
        (
            "verification started and not completed",
            "pending@example.com",
            VerificationStatus::Pending,
            false,
        ),
        (
            "verification failed",
            "failed@example.com",
            VerificationStatus::Failed,
            false,
        ),
        (
            "verification failed for a reason that may clear",
            "temporary@example.com",
            VerificationStatus::TemporaryFailure,
            false,
        ),
        (
            "verification never attempted",
            "notstarted@example.com",
            VerificationStatus::NotStarted,
            false,
        ),
        (
            "verification succeeded",
            "success@example.com",
            VerificationStatus::Success,
            true,
        ),
    ];
    {
        let store = state::service_state(&*engine).expect("this service has a store on the engine");
        let mut store = store.lock();
        for (_, address, status, _) in seeded {
            assert!(
                store
                    .verify_identity(address, IdentityType::EmailAddress)
                    .is_some(),
                "seeding {address:?}: the identity list must hold it"
            );
            let identity = store
                .identities
                .get_mut(*address)
                .expect("the identity just recorded is in the list");
            identity.verification_status = *status;
            // A record in any state but `Pending` has no move left for the clock to
            // make, so it carries no due instant; the pending row keeps the one the
            // request started, which this suite's instants are all inside.
            if *status != VerificationStatus::Pending {
                identity.verification_due = None;
            }
        }
    }

    // Every row names an identity this account HOLDS, differing only in the state the
    // list holds for it: only the one whose verification succeeded carries a template.
    for (case, address, status, accepted) in seeded {
        let template = Template::new(&format!("verify-from-{}", status.as_wire()), address);
        let (http_status, body) = create(&router, &template).await;
        if *accepted {
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected 200 for a verified sender, got {http_status} with {body}"
            );
            assert_stored(&router, case, &template).await;
            continue;
        }
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected 400 for a sender that cannot send, got {http_status} with \
             {body}"
        );
        assert_eq!(
            (code(&body).as_str(), message(&body).as_str()),
            (
                "FromEmailAddressNotVerified",
                format!("The from email address {address} is not verified.").as_str()
            ),
            "case {case:?}: expected the unverified-sender refusal, got {body}"
        );
        assert_absent(&router, case, &template.name).await;
    }

    // The counterpart, on the same store: an address NO identity covers is refused the
    // same way, so the gate is about the sender's verification and not about nothing at
    // all.
    let absent = Template::new("verify-from-no-identity", "nobody@example.org");
    let (http_status, body) = create(&router, &absent).await;
    assert_eq!(
        http_status, 400,
        "case an address with no identity: expected 400, got {http_status} with {body}"
    );
    assert_eq!(
        (code(&body).as_str(), message(&body).as_str()),
        (
            "FromEmailAddressNotVerified",
            "The from email address nobody@example.org is not verified."
        ),
        "case an address with no identity: expected the unverified-sender refusal, got {body}"
    );
    assert_absent(&router, "an address with no identity", &absent.name).await;
}

#[tokio::test]
async fn a_name_the_store_already_holds_is_refused_as_a_conflict() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;

    // The rows differ in how much of the second request repeats the first: the
    // identical call, one differing in every member but the name, and one differing
    // only in its sender. The name alone decides the conflict.
    let stored = Template::new("verify-once", "verification@example.com");
    let (status, body) = create(&router, &stored).await;
    assert_eq!(
        status, 200,
        "case the first create: expected 200, got {status} with {body}"
    );
    verify_email_identity(&router, &engine, "second@example.com").await;
    let rows: Vec<(&str, Template)> = vec![
        ("the identical call again", stored.clone()),
        (
            "every member but the name replaced",
            Template {
                subject: "Something else entirely",
                content: "<html><body>Different.</body></html>".to_string(),
                success_url: "https://example.com/other-ok",
                failure_url: "https://example.com/other-no",
                ..Template::new("verify-once", "second@example.com")
            },
        ),
        (
            "only the sender replaced",
            Template::new("verify-once", "second@example.com"),
        ),
    ];
    for (case, repeat) in &rows {
        let (status, body) = create(&router, repeat).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            (code(&body).as_str(), message(&body).as_str()),
            (
                "CustomVerificationEmailTemplateAlreadyExists",
                "Custom verification email template <verify-once> already exists."
            ),
            "case {case:?}: expected the naming conflict, got {body}"
        );
        // The stored template is the one the first create wrote.
        assert_stored(&router, case, &stored).await;
    }
}

#[tokio::test]
async fn content_the_email_cannot_carry_is_refused_as_invalid_content() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;

    // The documented bound is on the size of the email, so content ON it and past it
    // is content no custom verification email can be built from, and content inside
    // it is stored. The rows are the bound itself and one byte either side of it.
    //
    // The accepted row is read back through the naming conflict rather than through
    // the reporting read: a second create of the same name is refused only if the
    // first one was stored, and asserting it that way keeps a bound-sized body out
    // of the response.
    let inside = Template {
        content: "c".repeat(MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES - 1),
        ..Template::new("verify-just-inside", "verification@example.com")
    };
    let (status, body) = create(&router, &inside).await;
    assert_eq!(
        status,
        200,
        "case content one byte inside the bound: expected 200, got {status} with {}",
        head(&body)
    );
    let repeat = Template {
        content: "<html><body>Short.</body></html>".to_string(),
        ..inside.clone()
    };
    let (status, body) = create(&router, &repeat).await;
    assert_eq!(
        (status, code(&body).as_str()),
        (400, "CustomVerificationEmailTemplateAlreadyExists"),
        "case content one byte inside the bound: expected the stored template to hold the name, \
         got {status} with {}",
        head(&body)
    );

    for (case, name, size) in [
        (
            "content on the bound",
            "verify-on-the-bound",
            MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES,
        ),
        (
            "content one byte past the bound",
            "verify-past-the-bound",
            MAX_CUSTOM_VERIFICATION_EMAIL_CONTENT_BYTES + 1,
        ),
    ] {
        let template = Template {
            content: "c".repeat(size),
            ..Template::new(name, "verification@example.com")
        };
        let (status, body) = create(&router, &template).await;
        assert_eq!(
            status,
            400,
            "case {case:?}: expected 400, got {status} with {}",
            head(&body)
        );
        assert_eq!(
            code(&body),
            "CustomVerificationEmailInvalidContent",
            "case {case:?}: expected the invalid-content refusal, got {}",
            head(&body)
        );
        assert_absent(&router, case, name).await;
    }
}

#[tokio::test]
async fn a_create_past_the_account_s_ceiling_is_refused_and_stores_nothing() {
    let (router, engine) = emulator_and_engine();
    verify_email_identity(&router, &engine, "verification@example.com").await;

    // Fill the store to the account's ceiling through the operation itself, so the
    // bound is the one a caller meets rather than one a test seeded.
    let names: Vec<String> = (0..MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES)
        .map(|slot| format!("verify-slot-{slot}"))
        .collect();
    for name in &names {
        let (status, body) = call(
            &router,
            "CreateCustomVerificationEmailTemplate",
            &[
                field("TemplateName", name.clone()),
                field("FromEmailAddress", "verification@example.com"),
                field("TemplateSubject", "Please verify your address"),
                field("TemplateContent", "<html><body>Verify.</body></html>"),
                field("SuccessRedirectionURL", "https://example.com/verified"),
                field("FailureRedirectionURL", "https://example.com/failed"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case filling slot {name:?}: expected 200, got {status} with {body}"
        );
    }

    let over = Template::new("verify-one-too-many", "verification@example.com");
    let (status, body) = create(&router, &over).await;
    assert_eq!(
        status, 400,
        "case one past the ceiling: expected 400, got {status} with {body}"
    );
    assert_eq!(
        (code(&body).as_str(), message(&body).as_str()),
        (
            "LimitExceeded",
            format!(
                "Custom verification email template limit exceeded: this account may hold {} \
                 custom verification email templates.",
                MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES
            )
            .as_str()
        ),
        "case one past the ceiling: expected the service-limit refusal, got {body}"
    );
    assert_absent(&router, "one past the ceiling", &over.name).await;

    // And the templates inside the ceiling are untouched by the refusal.
    for name in [&names[0], &names[MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES - 1]] {
        let (status, body) = get(&router, name).await;
        assert_eq!(
            status, 200,
            "case stored {name:?} after the refusal: expected 200, got {status} with {body}"
        );
    }
}
