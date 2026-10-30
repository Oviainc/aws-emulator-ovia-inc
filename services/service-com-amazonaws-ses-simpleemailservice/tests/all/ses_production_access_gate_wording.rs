// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusal of an account that has not been granted
//! production access, driven end to end over the awsQuery wire.
//!
//! One rule: while the account has not been granted production access, a send of a
//! custom verification email is refused HTTP 400 `ProductionAccessNotGranted` whose
//! message is ONE fixed sentence — it states the account-wide condition and then the
//! one action that lifts it — so no part of it comes from the request or from the
//! store, and two requests naming entirely different addresses, templates and
//! configuration sets are worded identically.
//!
//! This suite is about that WORDING and only about it, over an account whose store
//! holds nothing: the gate is read before anything the request names, so nothing has
//! to exist for it to fire. The rows therefore differ only along the forms the
//! request's members can arrive in — an address in angle brackets, one that is not an
//! address at all, the empty one; a template name carrying XML metacharacters, a very
//! long one, the empty one; a configuration set named or not. Which modelled cause the
//! gate PRECEDES, and what a granted account is served instead, are the subject of
//! `ses_custom_verification_email_send.rs`, which this suite does not repeat.

use crate::harness::{call, element, elements, emulator, error_envelope, field, Field};
use smithy_server_runtime::MultiServiceRouter;

/// The code SES answers while the account has not been granted production access
/// (the model's `awsQueryError` code for `ProductionAccessNotGrantedException`).
const NOT_GRANTED: &str = "ProductionAccessNotGranted";

/// The sentence SES answers for that error class, as a literal here: every word of it
/// is fixed text, so nothing in it comes from the request or the store — and its
/// second sentence ends without a closing full stop.
const NOT_GRANTED_MESSAGE: &str = "Production access not granted. Please request production \
                                   access prior to sending custom verification emails";

/// One `SendCustomVerificationEmail` request.
async fn send(
    router: &MultiServiceRouter,
    address: &str,
    template_name: &str,
    configuration_set: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = vec![
        field("EmailAddress", address),
        field("TemplateName", template_name),
    ];
    if let Some(configuration_set) = configuration_set {
        params.push(field("ConfigurationSetName", configuration_set));
    }
    call(router, "SendCustomVerificationEmail", &params).await
}

/// The email-address identities the account holds, as `ListIdentities` reports them.
async fn email_identities(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListIdentities",
        &[field("IdentityType", "EmailAddress")],
    )
    .await;
    assert_eq!(
        status, 200,
        "listing identities: expected 200, got {status} with {body}"
    );
    element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// The forms the request's members can arrive in: `(case, address, template name,
/// configuration set)`. None of them has to name anything the store holds, because the
/// gate is read before the request is resolved.
fn member_forms() -> Vec<(&'static str, String, String, Option<&'static str>)> {
    vec![
        (
            "a plain address and a plain template name",
            "new@example.org".to_string(),
            "confirm-your-address".to_string(),
            None,
        ),
        (
            "an address in angle brackets",
            "Someone <bracketed@example.org>".to_string(),
            "confirm-your-address".to_string(),
            None,
        ),
        (
            "an address that is not an address",
            "not-an-address".to_string(),
            "confirm-your-address".to_string(),
            None,
        ),
        (
            "the empty address",
            String::new(),
            "confirm-your-address".to_string(),
            None,
        ),
        (
            "a template name carrying XML metacharacters",
            "new@example.org".to_string(),
            "tmpl<&>\"'absent".to_string(),
            None,
        ),
        (
            "a template name of 512 characters",
            "new@example.org".to_string(),
            "absent-".repeat(64) + "x",
            None,
        ),
        (
            "the empty template name",
            "new@example.org".to_string(),
            String::new(),
            None,
        ),
        (
            "a named configuration set",
            "new@example.org".to_string(),
            "confirm-your-address".to_string(),
            Some("no-such-set"),
        ),
    ]
}

#[tokio::test]
async fn the_sandbox_refusal_is_one_sentence_carrying_nothing_from_the_request() {
    let router = emulator();
    let before = email_identities(&router).await;

    let mut stated: Vec<String> = Vec::new();
    for (case, address, template_name, configuration_set) in member_forms() {
        let (status, body) = send(&router, &address, &template_name, configuration_set).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (400, Some(NOT_GRANTED), Some(NOT_GRANTED_MESSAGE)),
            "case {case:?}: expected the sandbox refusal worded {NOT_GRANTED_MESSAGE:?}, got \
             {status} with {body}"
        );
        // The sentence has no variable part, so nothing the request named is in it.
        let said = message.unwrap_or_default();
        for (member, value) in [
            ("EmailAddress", address.as_str()),
            ("TemplateName", template_name.as_str()),
            ("ConfigurationSetName", configuration_set.unwrap_or_default()),
        ] {
            if !value.is_empty() {
                assert!(
                    !said.contains(value),
                    "case {case:?}: expected the message not to quote {member} {value:?}, got \
                     {said:?}"
                );
            }
        }
        stated.push(said);
    }
    // One wording across every form, compared row against row rather than each row
    // against a literal.
    assert!(
        stated.windows(2).all(|pair| pair[0] == pair[1]),
        "expected every gated request to be worded identically, got {stated:?}"
    );
    // And the gated calls recorded nothing: no address any of them named joined the
    // account's identity list.
    let after = email_identities(&router).await;
    assert_eq!(
        after, before,
        "expected the identity list untouched by the gated calls, got {after:?}"
    );
}
