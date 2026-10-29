// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a custom verification email whose sender it will not send
//! from, driven end to end over the awsQuery wire: form-urlencoded body in, XML out,
//! one shared engine behind every call.
//!
//! One rule, asserted over a SET of senders and over EVERY operation that gates a
//! custom verification email on one: a sender no verified identity of the account
//! covers is refused HTTP 400 `FromEmailAddressNotVerified` whose message is the
//! sentence `The from email address <sender> is not verified.` — fixed text, with the
//! sender taken from the request (`CreateCustomVerificationEmailTemplate`) or from the
//! stored template (`SendCustomVerificationEmail`) as its only variable part, stated
//! exactly as it arrived.
//!
//! The rows differ along what the rule generalises over: the FORM the sender arrives in
//! (a plain mailbox, a mailbox under a subdomain, the display-name form a `FromAddress`
//! may carry, a local part whose characters an XML body has to escape, an uppercase
//! mailbox, and a value that is no address at all) and WHICH operation reads it. The
//! positive control on the same store — one verified sender, both operations accepted —
//! is what keeps this a statement about the refusal class rather than about every call.

use crate::harness::{
    call, element, emulator_over, error_envelope, field, settle_verifications, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{self, CustomVerificationEmailTemplate};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The code SES answers a sender it will not send a custom verification email from.
const NOT_VERIFIED: &str = "FromEmailAddressNotVerified";

/// The code `GetCustomVerificationEmailTemplate` answers for a name the account holds
/// no template under, which is how this suite reads "nothing was persisted".
const DOES_NOT_EXIST: &str = "CustomVerificationEmailTemplateDoesNotExist";

/// The sentence the unverified-sender refusal states, with `sender` filling its one
/// variable part — the wording SES answers for this error class, whichever operation
/// applied the gate.
fn unverified_message(sender: &str) -> String {
    format!("The from email address {sender} is not verified.")
}

/// A router over an engine this suite holds, so the account record and the template
/// store can be seeded beside the calls made over the wire.
fn emulator_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
}

/// Take this account out of the Amazon SES sandbox, which is what
/// `SendCustomVerificationEmail` reads before it looks at anything the request names.
fn grant_production_access(engine: &Arc<engine::InMemoryEngine>) {
    let store = state::service_state(&**engine).expect("this service has a store on the engine");
    store.lock().account.production_access_granted = true;
}

/// Record a template whose own sender is `sender`, the way the creating operation does.
///
/// Seeded through the store because the create refuses exactly the senders this suite is
/// about, so the send's own reading of the gate could not be reached over the wire
/// otherwise.
fn seed_template(engine: &Arc<engine::InMemoryEngine>, name: &str, sender: &str) {
    let store = state::service_state(&**engine).expect("this service has a store on the engine");
    let mut store = store.lock();
    let stored = store.put_custom_verification_email_template(CustomVerificationEmailTemplate {
        template_name: name.to_string(),
        from_email_address: sender.to_string(),
        template_subject: "Please verify your address".to_string(),
        template_content: "<html><body>Please verify your address.</body></html>".to_string(),
        success_redirection_url: "https://example.com/verified".to_string(),
        failure_redirection_url: "https://example.com/failed".to_string(),
    });
    assert!(
        stored.is_some(),
        "seeding {name:?}: the store must hold a template sent from {sender:?}"
    );
}

/// The six members a create carries, in the order the model lists them.
fn create_params(name: &str, sender: &str) -> Vec<Field> {
    vec![
        field("TemplateName", name),
        field("FromEmailAddress", sender),
        field("TemplateSubject", "Please verify your address"),
        field(
            "TemplateContent",
            "<html><body>Please verify your address.</body></html>",
        ),
        field("SuccessRedirectionURL", "https://example.com/verified"),
        field("FailureRedirectionURL", "https://example.com/failed"),
    ]
}

/// `CreateCustomVerificationEmailTemplate`.
async fn create(router: &MultiServiceRouter, name: &str, sender: &str) -> (u16, String) {
    call(
        router,
        "CreateCustomVerificationEmailTemplate",
        &create_params(name, sender),
    )
    .await
}

/// `SendCustomVerificationEmail` of the template `name` to `recipient`.
async fn send(router: &MultiServiceRouter, name: &str, recipient: &str) -> (u16, String) {
    call(
        router,
        "SendCustomVerificationEmail",
        &[field("EmailAddress", recipient), field("TemplateName", name)],
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

/// Assert the account holds no template under `name` — the modelled absence, which is
/// how a refused create is read as having written nothing.
async fn assert_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = get(router, name).await;
    let (code, message) = error_envelope(&body);
    assert_eq!(
        (status, code.as_deref()),
        (400, Some(DOES_NOT_EXIST)),
        "case {case:?}: expected no template under {name:?}, got {status} with {message:?}"
    );
}

/// One row: a sender form, and the template name the rows about it use.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The sender neither a mailbox nor a domain identity of the account covers.
    sender: &'static str,
    /// The name this row's template is created (or seeded) under.
    name: &'static str,
}

/// The rows, spanning the forms a sender arrives in.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a plain mailbox",
            sender: "stranger@example.org",
            name: "wording-plain-mailbox",
        },
        Row {
            case: "a mailbox under a subdomain",
            sender: "noreply@mail.example.org",
            name: "wording-subdomain-mailbox",
        },
        Row {
            case: "the display-name form a FromAddress may carry",
            sender: "\"Verification\" <verification@example.org>",
            name: "wording-display-name",
        },
        Row {
            case: "a local part an XML body has to escape",
            sender: "sales&marketing@example.org",
            name: "wording-escaped-local-part",
        },
        Row {
            case: "an uppercase mailbox",
            sender: "Stranger@EXAMPLE.ORG",
            name: "wording-uppercase-mailbox",
        },
        Row {
            case: "a value that is no address at all",
            sender: "not-an-address",
            name: "wording-not-an-address",
        },
    ]
}

#[tokio::test]
async fn the_create_states_the_unverified_sender_in_one_sentence() {
    let (router, engine) = emulator_and_engine();
    // One verified mailbox on the store, so each row below differs from the accepted
    // sender only in the identity that would have to cover it.
    let verified = "verification@example.com";
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", verified)],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding {verified:?}: expected 200, got {status} with {body}"
    );
    settle_verifications(&engine);

    for row in rows() {
        let Row { case, sender, name } = row;
        let (status, body) = create(&router, name, sender).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(NOT_VERIFIED),
                Some(unverified_message(sender).as_str())
            ),
            "case {case:?}: expected the unverified-sender refusal naming {sender:?}, got \
             {status} with {body}"
        );
        assert_absent(&router, case, name).await;
    }

    // The control on the same store: the sender the account HAS verified is accepted, so
    // the sentence above states a refusal class and not every create.
    let (status, body) = create(&router, "wording-accepted", verified).await;
    assert_eq!(
        status, 200,
        "case the verified sender: expected 200 for {verified:?}, got {status} with {body}"
    );
    let (status, body) = get(&router, "wording-accepted").await;
    assert_eq!(
        (status, element(&body, "FromEmailAddress")),
        (200, Some(verified.to_string())),
        "case the verified sender: expected the read to echo {verified:?}, got {status} with \
         {body}"
    );
}

#[tokio::test]
async fn the_send_states_the_stored_templates_unverified_sender_in_the_same_sentence() {
    let (router, engine) = emulator_and_engine();
    grant_production_access(&engine);
    let verified = "verification@example.com";
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", verified)],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding {verified:?}: expected 200, got {status} with {body}"
    );
    settle_verifications(&engine);

    for row in rows() {
        let Row { case, sender, name } = row;
        seed_template(&engine, name, sender);
        let (status, body) = send(&router, name, "recipient@example.net").await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(NOT_VERIFIED),
                Some(unverified_message(sender).as_str())
            ),
            "case {case:?}: expected the unverified-sender refusal naming the stored sender \
             {sender:?}, got {status} with {body}"
        );
    }

    // The control: the same send of a template whose stored sender IS verified goes
    // through, so both operations state one sentence for one condition.
    seed_template(&engine, "wording-send-accepted", verified);
    let (status, body) = send(&router, "wording-send-accepted", "recipient@example.net").await;
    assert_eq!(
        status, 200,
        "case the verified stored sender: expected 200, got {status} with {body}"
    );
    assert!(
        element(&body, "MessageId").is_some_and(|id| !id.is_empty()),
        "case the verified stored sender: expected a message id, got {body}"
    );
}
