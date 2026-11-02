// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a custom verification email template the account holds
//! nothing under, driven end to end over the awsQuery wire: form-urlencoded body
//! in, XML out, one shared engine behind every call.
//!
//! One rule, asserted over a SET of names and over every SERVED operation that resolves a
//! custom verification email template by name: a name the account holds no template
//! under is refused HTTP 400 `CustomVerificationEmailTemplateDoesNotExist` whose
//! message is ONE fixed sentence — "Provided custom verification email template name
//! does not exist." — with no variable part at all, so it never quotes the name the
//! request asked for and two calls differing only in that name are worded
//! identically.
//!
//! The rows differ along what the rule generalises over: the FORM of the name that
//! misses (never created, another case of a stored one, a prefix of it, one with
//! trailing whitespace, the empty name, a name carrying XML metacharacters, a
//! non-ASCII name, a very long name), WHICH operation resolved it, and WHAT the store
//! holds while it is asked. The positive control on the same store — a name the store
//! does hold, served by every one of those operations — is what keeps this a statement
//! about the absence class rather than about every call.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The code SES answers for a custom verification email template name the account
/// holds nothing under (the model's `awsQueryError` code for
/// `CustomVerificationEmailTemplateDoesNotExistException`).
const DOES_NOT_EXIST: &str = "CustomVerificationEmailTemplateDoesNotExist";

/// The sentence SES answers for that error class, as a literal here: every word of it
/// is fixed text, so nothing in it comes from the request.
const ABSENT_MESSAGE: &str = "Provided custom verification email template name does not exist.";

/// The sender every template in this suite is sent from, verified on the store below.
const SENDER: &str = "verification@example.com";

/// The recipient a send names — a legal address, so the send reaches the template
/// lookup rather than being refused for the address.
const RECIPIENT: &str = "recipient@example.net";

/// The name the store DOES hold, which the misses below are measured against.
const STORED: &str = "stored-template";

/// The operations that resolve a custom verification email template by name and are
/// served today, each named for the assertion messages.
///
/// `DeleteCustomVerificationEmailTemplate` resolves one too, but it is still
/// scaffolded (it answers 501), so it cannot be driven over the wire here; it reads
/// the same wording once it is implemented.
const RESOLVERS: &[&str] = &[
    "GetCustomVerificationEmailTemplate",
    "SendCustomVerificationEmail",
    "UpdateCustomVerificationEmailTemplate",
];

/// Take this account out of the Amazon SES sandbox, which is what
/// `SendCustomVerificationEmail` reads before it looks at anything the request names.
fn grant_production_access(engine: &Arc<engine::InMemoryEngine>) {
    let store = state::service_state(&**engine).expect("this service has a store on the engine");
    store.lock().account.production_access_granted = true;
}

/// The six members a create carries, in the order the model lists them.
fn create_params(name: &str) -> Vec<Field> {
    vec![
        field("TemplateName", name),
        field("FromEmailAddress", SENDER),
        field("TemplateSubject", "Please verify your address"),
        field(
            "TemplateContent",
            "<html><body>Please verify your address.</body></html>",
        ),
        field("SuccessRedirectionURL", "https://example.com/verified"),
        field("FailureRedirectionURL", "https://example.com/failed"),
    ]
}

/// A router over an engine whose account may send, and whose sending identity is
/// verified, so every call below reaches the template lookup.
async fn sending_account() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_and_engine();
    grant_production_access(&engine);
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", SENDER)],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding {SENDER:?}: expected 200, got {status} with {body}"
    );
    // Asking for the verification only STARTS it; a template's own sender has to be a
    // verified identity, so the store is settled past the span the clock owes that
    // record rather than slept for.
    settle_verifications(&engine);
    (router, engine)
}

/// One request that resolves a template by `name`, through the operation `action`.
async fn resolve(router: &MultiServiceRouter, action: &str, name: &str) -> (u16, String) {
    match action {
        "GetCustomVerificationEmailTemplate" => {
            call(router, action, &[field("TemplateName", name)]).await
        }
        "SendCustomVerificationEmail" => {
            call(
                router,
                action,
                &[field("EmailAddress", RECIPIENT), field("TemplateName", name)],
            )
            .await
        }
        // The name is the only member an update needs: every other one is optional,
        // so the call resolves the template and nothing else.
        "UpdateCustomVerificationEmailTemplate" => {
            call(router, action, &[field("TemplateName", name)]).await
        }
        other => panic!("case {other:?}: this suite drives no such operation"),
    }
}

/// The names that MISS, spanning the forms a name can arrive in.
fn missing_names() -> Vec<(&'static str, String)> {
    vec![
        ("a name never created", "no-such-template".to_string()),
        ("another case of the stored name", STORED.to_uppercase()),
        ("a prefix of the stored name", "stored".to_string()),
        (
            "the stored name with trailing whitespace",
            format!("{STORED} "),
        ),
        (
            "the stored name with one character more",
            format!("{STORED}."),
        ),
        ("the empty name", String::new()),
        (
            "a name carrying XML metacharacters",
            "tmpl<&>\"'absent".to_string(),
        ),
        ("a non-ASCII name", "vorlage-bestätigung".to_string()),
        ("a name of 512 characters", "absent-".repeat(64) + "x"),
    ]
}

/// Assert one refusal: the status, the code, and the whole message.
fn assert_absence(case: &str, action: &str, name: &str, status: u16, body: &str) {
    let (code, message) = error_envelope(body);
    assert_eq!(
        (status, code.as_deref(), message.as_deref()),
        (400, Some(DOES_NOT_EXIST), Some(ABSENT_MESSAGE)),
        "case {case:?} through {action}: expected the modelled absence for {name:?} worded \
         {ABSENT_MESSAGE:?}, got {status} with {body}"
    );
    // The sentence has no variable part, so the name asked for is nowhere in it.
    let stated = message.unwrap_or_default();
    if !name.is_empty() {
        assert!(
            !stated.contains(name),
            "case {case:?} through {action}: expected the message not to quote {name:?}, got \
             {stated:?}"
        );
    }
}

#[tokio::test]
async fn every_resolver_states_an_absent_template_in_one_fixed_sentence() {
    let (router, _engine) = sending_account().await;
    // One template on the store, so each miss below differs from a name that IS held
    // only in the name itself.
    let (status, body) = call(
        &router,
        "CreateCustomVerificationEmailTemplate",
        &create_params(STORED),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding {STORED:?}: expected 200, got {status} with {body}"
    );

    for (case, name) in missing_names() {
        // The two operations' refusals are compared with each other as well as with
        // the sentence, so one wording is asserted and not two that merely agree with
        // a literal.
        let mut stated: Vec<String> = Vec::new();
        for action in RESOLVERS {
            let (status, body) = resolve(&router, action, &name).await;
            assert_absence(case, action, &name, status, &body);
            stated.push(error_envelope(&body).1.unwrap_or_default());
        }
        assert!(
            stated.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: expected every resolver to state one wording, got {stated:?}"
        );
    }

    // The positive control on the same store: the name the store HOLDS is served by
    // every one of them, so the sentence above states an absence and not every call.
    let (status, body) = resolve(&router, "GetCustomVerificationEmailTemplate", STORED).await;
    assert_eq!(
        (status, element(&body, "TemplateName")),
        (200, Some(STORED.to_string())),
        "case the stored name: expected the read to report it, got {status} with {body}"
    );
    let (status, body) = resolve(&router, "SendCustomVerificationEmail", STORED).await;
    assert_eq!(
        status, 200,
        "case the stored name: expected the send to go through, got {status} with {body}"
    );
    assert!(
        element(&body, "MessageId").is_some_and(|id| !id.is_empty()),
        "case the stored name: expected a message id, got {body}"
    );
    let (status, body) = resolve(&router, "UpdateCustomVerificationEmailTemplate", STORED).await;
    assert_eq!(
        status, 200,
        "case the stored name: expected the update to go through, got {status} with {body}"
    );
}

#[tokio::test]
async fn the_absence_sentence_does_not_depend_on_what_the_store_holds() {
    // An EMPTY template store answers the same sentence as one holding templates: the
    // wording is a property of the error class, not of the store or of the name.
    let (empty, _empty_engine) = sending_account().await;
    let (filled, _filled_engine) = sending_account().await;
    for name in [STORED, "another-stored-template"] {
        let (status, body) = call(
            &filled,
            "CreateCustomVerificationEmailTemplate",
            &create_params(name),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding {name:?}: expected 200, got {status} with {body}"
        );
    }

    for (case, router, name) in [
        ("an empty template store", &empty, "no-such-template"),
        (
            "an empty store asked for a name the other store holds",
            &empty,
            STORED,
        ),
        (
            "a store holding other templates",
            &filled,
            "no-such-template",
        ),
    ] {
        for action in RESOLVERS {
            let (status, body) = resolve(router, action, name).await;
            assert_absence(case, action, name, status, &body);
        }
    }
}
