// SPDX-License-Identifier: CC0-1.0

//! The sender gate of `CreateCustomVerificationEmailTemplate`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind them all.
//!
//! One rule, asserted over a SET of senders rather than the one a report cited: a custom
//! verification email template may only be created for a `FromEmailAddress` the account
//! can send from, which is an identity whose verification has SUCCEEDED — its own, or a
//! domain identity covering it. Asking for an identity's verification only STARTS it, so
//! an address whose verification is still outstanding is not eligible to send, and the
//! create is refused HTTP 400 `FromEmailAddressNotVerified` naming that address, with
//! nothing persisted: the following `GetCustomVerificationEmailTemplate` reports the
//! template does not exist. Once the account's verification of that same identity has
//! come through, the identical call lands and the read echoes the sender.
//!
//! The rows differ along what the rule generalises over: the form of the sender (a mailbox
//! that is an identity of its own, a mailbox a domain identity covers, a mailbox under a
//! subdomain of one, and the display-name form a `FromAddress` may arrive in), and whether
//! the covering identity's verification has come through yet.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications, xml_text,
    Field,
};
use smithy_server_runtime::MultiServiceRouter;

/// The code SES answers a sender it will not send a custom verification email from.
const NOT_VERIFIED: &str = "FromEmailAddressNotVerified";

/// The code `GetCustomVerificationEmailTemplate` answers for a name the account holds no
/// template under, which is how this suite reads "nothing was persisted".
const DOES_NOT_EXIST: &str = "CustomVerificationEmailTemplateDoesNotExist";

/// One row: a sender, and the identity the account verifies for it.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The `FromEmailAddress` the create carries.
    sender: &'static str,
    /// The address the account asks SES to verify, which may be the sender's own mailbox
    /// or a domain that covers it.
    verifies: &'static str,
    /// Whether what the account verifies is a domain rather than a mailbox.
    is_domain: bool,
}

/// The rows, spanning the forms a covered sender takes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a mailbox that is an identity of its own",
            sender: "verification@example.com",
            verifies: "verification@example.com",
            is_domain: false,
        },
        Row {
            case: "a mailbox a domain identity covers",
            sender: "noreply@example.net",
            verifies: "example.net",
            is_domain: true,
        },
        Row {
            case: "a mailbox under a subdomain of a verified domain",
            sender: "noreply@mail.example.org",
            verifies: "example.org",
            is_domain: true,
        },
        Row {
            case: "the display-name form of a mailbox identity",
            sender: "\"Verification\" <verification@example.io>",
            verifies: "verification@example.io",
            is_domain: false,
        },
    ]
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

/// `GetCustomVerificationEmailTemplate`.
async fn get(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "GetCustomVerificationEmailTemplate",
        &[field("TemplateName", name)],
    )
    .await
}

/// The `FromEmailAddress` the read reports for `name`, and `None` when the account holds
/// no template under that name at all.
async fn stored_sender(router: &MultiServiceRouter, case: &str, name: &str) -> Option<String> {
    let (status, body) = get(router, name).await;
    if status == 200 {
        return Some(
            element(&body, "FromEmailAddress")
                .map(|raw| xml_text(&raw))
                .unwrap_or_default(),
        );
    }
    let (code, message) = error_envelope(&body);
    assert_eq!(
        (status, code.as_deref()),
        (400, Some(DOES_NOT_EXIST)),
        "case {case:?}: expected the modelled absence of {name:?}, got {status} with \
         {message:?}"
    );
    None
}

#[tokio::test]
async fn a_template_is_created_only_once_its_senders_verification_has_come_through() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = emulator_and_engine();
        // The account ASKS SES to verify the identity, which only starts the
        // verification: the move the clock owes the record has not fallen due, so the
        // sender cannot send yet.
        let (status, body) = if row.is_domain {
            call(&router, "VerifyDomainIdentity", &[field("Domain", row.verifies)]).await
        } else {
            call(
                &router,
                "VerifyEmailIdentity",
                &[field("EmailAddress", row.verifies)],
            )
            .await
        };
        assert_eq!(
            status, 200,
            "case {case:?}: asking to verify {:?} expected 200, got {status} with {body}",
            row.verifies
        );

        let name = "verify-while-pending";
        let (status, body) = create(&router, name, row.sender).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(NOT_VERIFIED),
                Some(format!("The from email address {} is not verified.", row.sender).as_str())
            ),
            "case {case:?}: expected the unverified-sender refusal naming {:?}, got {status} \
             with {body}",
            row.sender
        );
        // Refused before anything was written: the name is still free.
        let stored = stored_sender(&router, case, name).await;
        assert_eq!(
            stored, None,
            "case {case:?}: a refused create must persist nothing, got {stored:?}"
        );

        // The identical call once the account's verification of that identity has come
        // through. Only the verification differs — the request is the same one — and now
        // it lands and reads back.
        settle_verifications(&engine);
        let (status, body) = create(&router, name, row.sender).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once {:?} is verified, got {status} with {body}",
            row.verifies
        );
        let stored = stored_sender(&router, case, name).await;
        assert_eq!(
            stored.as_deref(),
            Some(row.sender),
            "case {case:?}: expected the read to echo the sender {:?}, got {stored:?}",
            row.sender
        );
    }
}

#[tokio::test]
async fn a_sender_no_verified_identity_covers_is_refused_and_persists_nothing() {
    let (router, engine) = emulator_and_engine();
    // One verified mailbox and one verified domain, carried through to SENDING
    // identities, so each row below differs from an accepted sender only in the identity
    // that would have to cover it.
    for (action, member, value) in [
        ("VerifyEmailIdentity", "EmailAddress", "sender@example.com"),
        ("VerifyDomainIdentity", "Domain", "mail.example.net"),
    ] {
        let (status, body) = call(&router, action, &[field(member, value)]).await;
        assert_eq!(
            status, 200,
            "seeding {value:?}: expected 200, got {status} with {body}"
        );
    }
    settle_verifications(&engine);

    // The rows differ in HOW the sender fails the gate: a domain the account holds no
    // identity for, a domain that only resembles a verified one, the PARENT of a verified
    // domain (a verified domain covers what is under it, never what is above it), another
    // mailbox at the unverified domain of a verified mailbox, and a value that is no
    // address at all.
    let rows: &[(&str, &str, &str)] = &[
        (
            "a domain with no identity",
            "no-identity",
            "sender@example.org",
        ),
        (
            "a domain that only resembles a verified one",
            "lookalike",
            "sender@notmail.example.net",
        ),
        (
            "the parent of a verified domain",
            "parent-domain",
            "sender@example.net",
        ),
        (
            "another mailbox at the unverified domain of a verified mailbox",
            "other-mailbox",
            "someone-else@example.com",
        ),
        (
            "a sender that is no address at all",
            "not-an-address",
            "not-an-address",
        ),
    ];
    for (case, name, sender) in rows {
        let (status, body) = create(&router, name, sender).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(NOT_VERIFIED),
                Some(format!("The from email address {sender} is not verified.").as_str())
            ),
            "case {case:?}: expected the unverified-sender refusal naming {sender:?}, got \
             {status} with {body}"
        );
        let stored = stored_sender(&router, case, name).await;
        assert_eq!(
            stored, None,
            "case {case:?}: a refused create must persist nothing, got {stored:?}"
        );
    }

    // The control on the same store: a sender the verified mailbox and the verified
    // domain do cover is accepted, so the gate is about the sender and not about every
    // create.
    for (case, name, sender) in [
        ("the verified mailbox", "accepted-mailbox", "sender@example.com"),
        (
            "a mailbox the verified domain covers",
            "accepted-domain",
            "noreply@mail.example.net",
        ),
    ] {
        let (status, body) = create(&router, name, sender).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {sender:?}, got {status} with {body}"
        );
        let stored = stored_sender(&router, case, name).await;
        assert_eq!(
            stored.as_deref(),
            Some(sender),
            "case {case:?}: expected the read to echo {sender:?}, got {stored:?}"
        );
    }
}
