// SPDX-License-Identifier: CC0-1.0

//! A domain identity's TXT verification token, end to end over the wire.
//!
//! `VerifyDomainIdentity` hands back the value the customer publishes as the TXT
//! record at `_amazonses.<domain>`, and `GetIdentityVerificationAttributes` reports
//! it for the identity afterwards. What is asserted is the RULE the token follows
//! over a SET of domains, not one recorded token:
//!
//! * it is base64 whose DECODED value is exactly [`VERIFICATION_TOKEN_BYTES`] bytes —
//!   the length Amazon SES issues, and the one structural property a caller can check
//!   before it puts the record in DNS;
//! * it is STORED, so re-verifying the domain and reading the identity back both
//!   answer the value the owner was told to publish, rather than a value minted per
//!   call;
//! * no two domains are given the same one; and
//! * an email-address identity carries none at all
//!   (`IdentityVerificationAttributes$VerificationToken`: "The verification token for
//!   a domain identity. Null for email address identities.").

use crate::harness::{call, element, field, map_entries, Field};
use smithy_server_runtime::MultiServiceRouter;

/// The decoded length of an Amazon SES verification token, as the shared identity
/// registry mints it (`service_common::ses_identities`).
const VERIFICATION_TOKEN_BYTES: usize = 32;

/// `VerifyDomainIdentity`, as the token it answers with.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) -> String {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "VerifyDomainIdentity({domain:?}): expected 200, got {status} with {body}"
    );
    element(&body, "VerificationToken")
        .unwrap_or_else(|| panic!("VerifyDomainIdentity({domain:?}): expected a token, got {body}"))
}

/// `GetIdentityVerificationAttributes`, as `(status, token)` for ONE identity.
async fn reported(router: &MultiServiceRouter, identity: &str) -> (String, Option<String>) {
    let params: Vec<Field> = vec![field("Identities.member.1", identity)];
    let (status, body) = call(router, "GetIdentityVerificationAttributes", &params).await;
    assert_eq!(
        status, 200,
        "GetIdentityVerificationAttributes({identity:?}): expected 200, got {status} with {body}"
    );
    let attributes = element(&body, "VerificationAttributes").unwrap_or_default();
    let entries = map_entries(&attributes);
    let (_, value) = entries.first().unwrap_or_else(|| {
        panic!("GetIdentityVerificationAttributes({identity:?}): expected one entry, got {body}")
    });
    (
        element(value, "VerificationStatus").unwrap_or_default(),
        element(value, "VerificationToken"),
    )
}

/// The decoded bytes of a token, or the reason it is not base64 at all.
fn decoded(token: &str) -> Result<Vec<u8>, String> {
    ::aws_smithy_types::base64::decode(token).map_err(|error| error.to_string())
}

#[tokio::test]
async fn a_domain_verification_token_is_base64_over_thirty_two_bytes_and_is_stored() {
    let router = crate::harness::emulator();
    // The rule generalises over the NAME, so the rows differ in label count, label
    // length, case and top-level domain — the recorded case is one row.
    let domains = [
        "example.com",
        "mail.example.com",
        "deep.sub.example.org",
        "MiXeD.Example.NET",
        "a.io",
        "a-very-long-label-for-one-dns-name.example.co.uk",
    ];
    let mut minted: Vec<(&str, String)> = Vec::new();
    for domain in domains {
        let token = verify_domain(&router, domain).await;
        let bytes = decoded(&token)
            .unwrap_or_else(|why| panic!("case {domain:?}: token {token:?} is not base64: {why}"));
        assert_eq!(
            bytes.len(),
            VERIFICATION_TOKEN_BYTES,
            "case {domain:?}: expected the token to decode to {VERIFICATION_TOKEN_BYTES} bytes, \
             got {} from {token:?}",
            bytes.len()
        );

        // Stored, not minted per call: asking again answers the same value, and so
        // does the identity report.
        let again = verify_domain(&router, domain).await;
        assert_eq!(
            again, token,
            "case {domain:?}: re-verifying must answer the stored token {token:?}, got {again:?}"
        );
        let (status, reported_token) = reported(&router, domain).await;
        assert_eq!(
            reported_token.as_deref(),
            Some(token.as_str()),
            "case {domain:?}: the identity report must carry the stored token {token:?}, got \
             {reported_token:?}"
        );
        // Requesting verification only STARTS it, so the freshly requested domain is
        // reported as pending while the clock owes it its move.
        assert_eq!(
            status, "Pending",
            "case {domain:?}: a domain whose TXT record nobody has published yet reads Pending, \
             got {status:?}"
        );
        minted.push((domain, token));
    }

    for (domain, token) in &minted {
        let collisions: Vec<&str> = minted
            .iter()
            .filter(|(other, other_token)| other != domain && other_token == token)
            .map(|(other, _)| *other)
            .collect();
        assert!(
            collisions.is_empty(),
            "case {domain:?}: expected a token of its own, got {token:?} shared with \
             {collisions:?}"
        );
    }
}

#[tokio::test]
async fn an_email_address_identity_carries_no_verification_token() {
    let router = crate::harness::emulator();
    // The rule holds for every address, so the rows differ in local part, domain and
    // case rather than naming one recorded address.
    let addresses = [
        "user@example.com",
        "user.name+tag@mail.example.com",
        "MiXeD@Example.NET",
    ];
    for address in addresses {
        let (status, body) = call(
            &router,
            "VerifyEmailIdentity",
            &[field("EmailAddress", address)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {address:?}: expected 200, got {status} with {body}"
        );
        let (reported_status, token) = reported(&router, address).await;
        assert!(
            token.is_none(),
            "case {address:?}: an address identity carries no VerificationToken, got {token:?}"
        );
        assert_eq!(
            reported_status, "Pending",
            "case {address:?}: a freshly requested address reads Pending, got {reported_status:?}"
        );
    }
}
