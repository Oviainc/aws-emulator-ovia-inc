// SPDX-License-Identifier: CC0-1.0

//! WHICH identity the unverified-sender `MessageRejected` names, over the forms a
//! `Source` may be written in.
//!
//! The Region half of that refusal is pinned by `ses_unverified_sender_region.rs`;
//! this suite pins the other variable part of the same sentence: the identities it
//! lists are the ones the send checked, and the identity a send checks is the
//! ADDR-SPEC of its `Source` — `Source` "may be in one of two forms: either
//! `User Name <sender@example.com>` or `sender@example.com`" — so the mailbox a
//! display name wraps is both what the gate reads and what the refusal names. The
//! rows differ along the form the same mailbox arrives in (bare, wrapped in a
//! display name, a quoted display name, surrounding whitespace), and each row's
//! message therefore names the one mailbox rather than the text submitted.

use crate::harness::{
    call, element, emulator, emulator_and_engine, field, settle_verifications, xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::DEFAULT_REGION;
use smithy_server_runtime::MultiServiceRouter;

/// The message a send from an unverified identity is refused with, as this service
/// words it for the Region the harness's calls are scoped to and one identity.
fn expected_message(identity: &str) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {identity}",
        DEFAULT_REGION.to_uppercase()
    )
}

/// `SendEmail` from `source` to one recipient.
async fn send_from(router: &MultiServiceRouter, source: &str) -> (u16, String) {
    let params: Vec<Field> = vec![
        field("Source", source),
        field("Destination.ToAddresses.member.1", "recipient@example.net"),
        field("Message.Subject.Data", "Quarterly report"),
        field("Message.Body.Text.Data", "See attached."),
    ];
    call(router, "SendEmail", &params).await
}

#[tokio::test]
async fn the_refusal_names_the_addr_spec_of_the_source_whatever_form_it_arrived_in() {
    // Every row carries the SAME mailbox, written differently; the refusal names the
    // mailbox, never the display name or the angle brackets around it.
    let rows = [
        ("a bare mailbox", "stranger@example.com"),
        ("a display name and an angle-addr", "Sender <stranger@example.com>"),
        (
            "a quoted display name",
            "\"Sender, Sales\" <stranger@example.com>",
        ),
        ("whitespace around the whole value", "  stranger@example.com  "),
        (
            "whitespace inside the angle-addr",
            "Sender < stranger@example.com >",
        ),
    ];

    for (case, source) in rows {
        let router = emulator();
        // One verified identity, so each refusal is about the Source named rather
        // than about an empty store.
        let (status, body) = call(
            &router,
            "VerifyEmailIdentity",
            &[field("EmailAddress", "allowed@example.com")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: VerifyEmailIdentity expected 200, got {status} with {body}"
        );

        let (status, body) = send_from(&router, source).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unverified source is refused with 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("MessageRejected"),
            "case {case:?}: expected the modelled MessageRejected code, got {body}"
        );
        let stated = element(&body, "Message")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default();
        let expected = expected_message("stranger@example.com");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_same_forms_of_a_verified_identity_are_accepted() {
    // The other half of the rule: the gate reads the addr-spec too, so each form of a
    // mailbox this account HAS verified sends rather than being refused — the refusal
    // above is about the identity, not about the form it was written in.
    for (case, source) in [
        ("a bare mailbox", "allowed@example.com"),
        ("a display name and an angle-addr", "Sender <allowed@example.com>"),
        ("whitespace around the whole value", " allowed@example.com "),
    ] {
        let (router, engine) = emulator_and_engine();
        let (status, body) = call(
            &router,
            "VerifyEmailIdentity",
            &[field("EmailAddress", "allowed@example.com")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: VerifyEmailIdentity expected 200, got {status} with {body}"
        );
        // The request only ASKS for the verification; these rows are about the form
        // the Source arrives in, so the store is settled past the span the clock owes
        // the record rather than slept through.
        settle_verifications(&engine);

        let (status, body) = send_from(&router, source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a verified source is accepted, got {status} with {body}"
        );
        assert!(
            element(&body, "MessageId").is_some(),
            "case {case:?}: an accepted send reports the id it stored, got {body}"
        );
    }
}
