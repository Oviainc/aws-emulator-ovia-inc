// SPDX-License-Identifier: CC0-1.0

//! `VerifyEmailAddress` — the deprecated spelling of `VerifyEmailIdentity` —
//! end to end over the wire, through `MultiServiceRouter::dispatch`, so every
//! call here is a real awsQuery request: form-urlencoded body in, XML out, one
//! shared engine behind all of them.
//!
//! The RULE asserted, over a SET of inputs rather than one address: a call
//! records the MAILBOX the request names — its addr-spec, verbatim — in the
//! account's ONE identity list as an email-address identity, and every reader
//! of that list reports it afterwards (`ListVerifiedEmailAddresses`,
//! `ListIdentities`, `GetIdentityVerificationAttributes`); the write is
//! idempotent, so the list never doubles up and a second call through the
//! CURRENT spelling lands on the same record; an email-address identity carries
//! no verification token, which the model states for
//! `IdentityVerificationAttributes$VerificationToken` ("The verification token
//! for a domain identity. Null for email address identities."); and verifying a
//! mailbox verifies that mailbox alone, never the domain it sits in.
//!
//! A value that is not an address in the form SES documents
//! (`UserName@[SubDomain.]Domain.TopLevelDomain`, 7-bit ASCII) is refused, and a
//! refused call records nothing — the deprecated spelling reads the same form as
//! the current one. The refusal is asserted as this crate's shared
//! client-refusal channel words it (`crate::scaffolded::Scaffolded::validation`,
//! which `verify_email_identity.rs` answers the same rule with), so a later
//! change to that wording moves this assertion with it.
//!
//! What this suite deliberately does NOT pin is WHICH `VerificationStatus` a
//! freshly verified address is reported with: it asserts only that the reported
//! value is one the model's enum declares. That choice belongs to the shared
//! write path both spellings use (`state::verify_identity`), not to this
//! operation, so it is left to its own issue rather than fixed in place here.
//! The address listing reports the addresses whose verification has GONE
//! THROUGH, and the call only asks for it, so the rows settle the store past the
//! span the clock owes each record (`harness::settle_verifications`) instead of
//! waiting for it.

use crate::harness::{
    call, element, elements, emulator, emulator_and_engine, field, map_entries,
    settle_verifications, Field,
};
use service_com_amazonaws_ses_simpleemailservice::ops::types::VERIFICATION_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// One call that must succeed, as its XML body.
async fn ok(router: &MultiServiceRouter, action: &str, params: &[Field]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {action}: expected 200, got {status} with {body}"
    );
    body
}

/// `VerifyEmailAddress`, the operation under test.
async fn verify_address(router: &MultiServiceRouter, address: &str) -> (u16, String) {
    call(
        router,
        "VerifyEmailAddress",
        &[field("EmailAddress", address)],
    )
    .await
}

/// `ListVerifiedEmailAddresses`, as the addresses it reports.
async fn verified_addresses(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListVerifiedEmailAddresses", &[]).await;
    element(&body, "VerifiedEmailAddresses")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// `ListIdentities`, as the identities it reports — every class when
/// `identity_type` is `None`, otherwise just that class.
async fn identities(router: &MultiServiceRouter, identity_type: Option<&str>) -> Vec<String> {
    let params: Vec<Field> = identity_type
        .map(|class| vec![field("IdentityType", class)])
        .unwrap_or_default();
    let body = ok(router, "ListIdentities", &params).await;
    element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// `GetIdentityVerificationAttributes` for one identity, as
/// `(the reported status, whether a verification token was reported)` — or
/// `None` when the account holds no such identity, which is how the operation
/// reports one it does not know: an absent map entry rather than an invented
/// status.
async fn verification_attributes(
    router: &MultiServiceRouter,
    identity: &str,
) -> Option<(String, bool)> {
    let body = ok(
        router,
        "GetIdentityVerificationAttributes",
        &[field("Identities.member.1", identity)],
    )
    .await;
    let map = element(&body, "VerificationAttributes").unwrap_or_default();
    let entries = map_entries(&map);
    assert!(
        entries.len() <= 1,
        "case {identity:?}: one requested identity cannot yield more than one entry, got \
         {entries:?}"
    );
    entries.into_iter().next().map(|(key, value)| {
        assert_eq!(
            key, identity,
            "case {identity:?}: the entry must be keyed by the identity asked about, got {key:?}"
        );
        let status = element(&value, "VerificationStatus").unwrap_or_else(|| {
            panic!("case {identity:?}: an attributes entry must carry VerificationStatus, got {value}")
        });
        (status, element(&value, "VerificationToken").is_some())
    })
}

#[tokio::test]
async fn verify_email_address_records_the_mailbox_every_reader_reports() {
    let (router, engine) = emulator_and_engine();

    assert!(
        verified_addresses(&router).await.is_empty(),
        "case fresh emulator: expected no verified addresses"
    );

    // Each row differs in the ONE thing the rule generalises over: the FORM the
    // request's address takes. A dot-atom local part, a tagged one, the atom
    // punctuation RFC 5322 admits, a mailbox under a subdomain, the shortest
    // labels that are still an address, letter case (carried verbatim — SES
    // reports back the identity it was given), and the RFC 5322 display-name
    // form, where what is recorded is the addr-spec and never the display text.
    let rows: &[(&str, &str, &str)] = &[
        ("plain mailbox", "ops@example.com", "ops@example.com"),
        (
            "dotted local part with a tag",
            "first.last+ses@example.org",
            "first.last+ses@example.org",
        ),
        (
            "atom punctuation",
            "a+b_c-d{e}@example.com",
            "a+b_c-d{e}@example.com",
        ),
        (
            "mailbox under a subdomain",
            "alerts@mail.corp.example.net",
            "alerts@mail.corp.example.net",
        ),
        ("shortest labels", "a@b.co", "a@b.co"),
        (
            "letter case carried verbatim",
            "Ops.Team@Example.COM",
            "Ops.Team@Example.COM",
        ),
        (
            "display-name form",
            "\"Ops Team\" <team@example.com>",
            "team@example.com",
        ),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, requested, mailbox) in rows {
        // Twice, because the write is idempotent: the second call must leave the
        // one record — and the one listing entry — alone.
        for attempt in 1..=2 {
            let (status, body) = verify_address(&router, requested).await;
            assert_eq!(
                status, 200,
                "case {case} attempt {attempt}: expected 200, got {status} with {body}"
            );
            settle_verifications(&engine);

            let listed = verified_addresses(&router).await;
            assert_eq!(
                listed.iter().filter(|entry| *entry == mailbox).count(),
                1,
                "case {case} attempt {attempt}: expected exactly one entry for {mailbox:?}, got \
                 {listed:?}"
            );
            assert!(
                !listed.iter().any(|entry| entry.contains("Ops Team")),
                "case {case} attempt {attempt}: the display text is not an identity, got {listed:?}"
            );
            assert!(
                identities(&router, None)
                    .await
                    .contains(&(*mailbox).to_string()),
                "case {case} attempt {attempt}: expected ListIdentities to report {mailbox:?}"
            );

            // The round trip: the write is readable back through the per-identity
            // reporting operation, with a status the model's enum declares and no
            // verification token, which is null for an email-address identity.
            let (reported_status, has_token) = verification_attributes(&router, mailbox)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {case} attempt {attempt}: expected attributes for {mailbox:?}, got \
                         none"
                    )
                });
            assert!(
                // The accepted spellings come from the model's own
                // `VerificationStatus` enum, published by this crate as
                // `ops::types::VERIFICATION_STATUS_VALUES`
                // (smithy_query(query="shape", shape="VerificationStatus")).
                VERIFICATION_STATUS_VALUES.contains(&reported_status.as_str()),
                "case {case} attempt {attempt}: {reported_status:?} is not a VerificationStatus \
                 the model declares, expected one of {VERIFICATION_STATUS_VALUES:?}"
            );
            assert!(
                !has_token,
                "case {case} attempt {attempt}: an email-address identity carries no verification \
                 token, got a token with status {reported_status:?}"
            );
        }
        expected.push((*mailbox).to_string());
    }

    expected.sort();
    let listed = verified_addresses(&router).await;
    assert_eq!(
        listed, expected,
        "case every form verified: expected each mailbox once, in one stable order, got {listed:?}"
    );

    // Verifying a mailbox verifies the MAILBOX: the domain it sits in is not an
    // identity of this account, so no reader reports one.
    let domains = identities(&router, Some("Domain")).await;
    assert!(
        domains.is_empty(),
        "case mailboxes only: expected no domain identity, got {domains:?}"
    );
    for domain in ["example.com", "example.org", "mail.corp.example.net"] {
        let reported = verification_attributes(&router, domain).await;
        assert!(
            reported.is_none(),
            "case domain {domain:?}: a mailbox verification does not verify its domain, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_deprecated_and_current_spellings_write_one_record() {
    let (router, engine) = emulator_and_engine();

    // Both operations verify an email address, so whichever order a caller uses
    // them in, the account ends up holding ONE record for that mailbox. The rows
    // span the orders, including each spelling used alone twice.
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "deprecated then current",
            "both-orders-a@example.com",
            &["VerifyEmailAddress", "VerifyEmailIdentity"],
        ),
        (
            "current then deprecated",
            "both-orders-b@example.com",
            &["VerifyEmailIdentity", "VerifyEmailAddress"],
        ),
        (
            "deprecated twice",
            "deprecated-only@example.com",
            &["VerifyEmailAddress", "VerifyEmailAddress"],
        ),
        (
            "alternating four times",
            "alternating@example.com",
            &[
                "VerifyEmailAddress",
                "VerifyEmailIdentity",
                "VerifyEmailAddress",
                "VerifyEmailIdentity",
            ],
        ),
    ];
    for (case, mailbox, actions) in rows {
        for action in *actions {
            ok(&router, action, &[field("EmailAddress", *mailbox)]).await;
        }
        settle_verifications(&engine);
        let listed = verified_addresses(&router).await;
        assert_eq!(
            listed.iter().filter(|entry| entry == mailbox).count(),
            1,
            "case {case}: {actions:?} must leave exactly one entry for {mailbox:?}, got {listed:?}"
        );
        assert!(
            verification_attributes(&router, mailbox).await.is_some(),
            "case {case}: expected attributes for {mailbox:?} after {actions:?}"
        );
    }

    // And one record is what the deprecated removal takes away, whichever
    // spelling wrote it: the mailbox is gone from both readers afterwards.
    for (case, mailbox, _) in rows {
        ok(
            &router,
            "DeleteVerifiedEmailAddress",
            &[field("EmailAddress", *mailbox)],
        )
        .await;
        let listed = verified_addresses(&router).await;
        assert!(
            !listed.contains(&(*mailbox).to_string()),
            "case {case}: expected {mailbox:?} gone after the deprecated removal, got {listed:?}"
        );
        let reported = verification_attributes(&router, mailbox).await;
        assert!(
            reported.is_none(),
            "case {case}: expected no attributes for the removed {mailbox:?}, got {reported:?}"
        );
    }
    let listed = verified_addresses(&router).await;
    assert!(
        listed.is_empty(),
        "case every mailbox removed: expected an empty listing, got {listed:?}"
    );
}

#[tokio::test]
async fn verify_email_address_refuses_a_value_that_is_not_an_email_address() {
    let router = emulator();

    // `EmailAddress` targets a bare string in the model
    // (smithy_query(query="shape", shape="Address")), so the boundary validator
    // admits every one of these and the handler is what reads the form SES
    // documents. Rows span the ways a value misses it: no mailbox at all, an
    // empty half, an empty atom of the dot-atom local part, whitespace, a second
    // `@`, a domain that is a single label or carries an empty one, a non-ASCII
    // local part, and the display-name form wrapping a non-address.
    let refused = [
        "",
        "not-an-address",
        "@example.com",
        "user@",
        "user@@example.com",
        "user name@example.com",
        "user@localhost",
        "user@example..com",
        "user@example.123",
        "üser@example.com",
        ".user@example.com",
        "user.@example.com",
        "user..name@example.com",
        "\"Ops Team\" <not-an-address>",
    ];
    for value in refused {
        let (status, body) = verify_address(&router, value).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("ValidationException"),
            "case {value:?}: expected a validation refusal, got {body}"
        );
        assert_eq!(
            element(&body, "Type").as_deref(),
            Some("Sender"),
            "case {value:?}: expected the refusal attributed to the caller, got {body}"
        );
    }

    // A refused call records NOTHING: no reader of the identity list reports any
    // of those values, and the list is still empty.
    let listed = verified_addresses(&router).await;
    assert!(
        listed.is_empty(),
        "case after only refused verifications: expected no verified addresses, got {listed:?}"
    );
    let all = identities(&router, None).await;
    assert!(
        all.is_empty(),
        "case after only refused verifications: expected no identities, got {all:?}"
    );
}
