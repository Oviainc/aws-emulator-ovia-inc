// SPDX-License-Identifier: CC0-1.0

//! `SendEmailVerificationCode` end to end over the wire: which addresses this
//! service sends a code to, what it persists for one, and how it refuses a
//! malformed address.
//!
//! Every call goes through the real [`MultiServiceRouter`] the production emulator
//! mounts, through the harness the lifecycle suite owns ([`Emulator`]). The
//! operation's modelled response carries NO member and the code it sends is
//! `@sensitive`, so what the call produced is read back out of the store rather than
//! out of the response — the response is checked for carrying none of it.
//!
//! Each test is a TABLE whose rows differ along what its rule generalises over: the
//! form of the address (local part, subdomain depth, top-level domain, case, length)
//! and the catalog it is sent in.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_partnercentralaccount_partnercentralaccount::state::{
    ComAmazonawsPartnercentralaccountPartnercentralaccountState as PartnerCentralState,
    EmailVerificationCodeRecord, EMAIL_VERIFICATION_CODE_TTL, MAX_VERIFICATION_EMAILS,
    VERIFICATION_CODE_DIGITS,
};

use crate::connection_lifecycle::{assert_error, error_code, string, Emulator};

/// `Email`'s modelled `@length` maximum (source:
/// `smithy_query(query="operation",operation="SendEmailVerificationCode")`).
const EMAIL_MAX_LEN: usize = 320;

/// A `SendEmailVerificationCode` request.
fn send(catalog: &str, email: &str) -> Value {
    json!({ "Catalog": catalog, "Email": email })
}

/// The verification code this service holds for `(catalog, email)`, if any.
///
/// The operation returns nothing, so this is the only place its effect can be
/// observed: no implemented operation spends a code yet (`CreatePartner` and
/// `AssociateAwsTrainingCertificationEmailDomain`, the two that take one, are still
/// scaffolded), so the store is read directly the same way the lifecycle suite
/// reaches it for the clock.
fn held(emulator: &Emulator, catalog: &str, email: &str) -> Option<EmailVerificationCodeRecord> {
    emulator
        .engine
        .state::<PartnerCentralState>()
        .lock()
        .email_verification_codes
        .get(&(catalog.to_string(), email.to_string()))
        .cloned()
}

/// How many addresses hold a code right now.
fn held_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<PartnerCentralState>()
        .lock()
        .email_verification_codes
        .len()
}

/// An address `Email`'s modelled constraints admit is sent a six-digit code, which
/// the service PERSISTS against the catalog and the address it was sent to, and
/// keeps out of the response.
///
/// Rows: the forms `Email` admits — a plain address, a dotted and tagged local part,
/// mixed case, the shortest local part and top-level domain, several subdomain
/// depths, a punycode top-level domain, a domain of a single label (the pattern
/// admits one, and this operation applies no address grammar the model does not
/// state), the published maximum length — crossed with the catalogs this service is
/// called in, including one a digit and a dash (`Catalog`, `^[a-zA-Z0-9-]+$`) spell:
/// this operation mints no ARN, so a catalog no ARN could carry is still an address
/// this service sends to.
#[tokio::test]
async fn an_address_the_model_admits_is_sent_a_six_digit_code() {
    let emulator = Emulator::new();
    let longest = format!("{}@example.com", "a".repeat(EMAIL_MAX_LEN - "@example.com".len()));

    // (case, catalog, address)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a plain address",
            "AWS",
            "partner@example.com".to_string(),
        ),
        (
            "a dotted, tagged local part and a two-label suffix",
            "Sandbox",
            "alliance.lead+partner@example.co.uk".to_string(),
        ),
        (
            "mixed case",
            "AWS",
            "Alliance.Lead@EXAMPLE.COM".to_string(),
        ),
        (
            "the shortest local part and top-level domain",
            "Sandbox",
            "a@b.io".to_string(),
        ),
        (
            "a deep subdomain and a long top-level domain",
            "AWS",
            "ops@mail.corp.example.technology".to_string(),
        ),
        (
            "a punycode top-level domain",
            "Sandbox",
            "partner!tag@sub-domain.example.xn--p1ai".to_string(),
        ),
        (
            "a catalog no ARN could carry",
            "catalog-1",
            "partner@example.org".to_string(),
        ),
        (
            "a domain of a single label",
            "AWS",
            "partner@example".to_string(),
        ),
        (
            "the longest address Email admits",
            "AWS",
            longest.clone(),
        ),
    ];

    let mut codes: BTreeSet<String> = BTreeSet::new();
    for (case, catalog, email) in &rows {
        let (status, body) = emulator.call("SendEmailVerificationCode", &send(catalog, email)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an address the model admits is sent a code, got {status}: {body}"
        );
        assert!(
            body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: the modelled response carries no member, got {body}"
        );

        let record = held(&emulator, catalog, email).unwrap_or_else(|| {
            panic!("case {case:?}: the code sent to {email} in {catalog} must be stored")
        });
        assert_eq!(
            record.code.len(),
            VERIFICATION_CODE_DIGITS,
            "case {case:?}: EmailVerificationCode is exactly {VERIFICATION_CODE_DIGITS} characters, got {:?}",
            record.code
        );
        assert!(
            record.code.bytes().all(|byte| byte.is_ascii_digit()),
            "case {case:?}: EmailVerificationCode is decimal digits, got {:?}",
            record.code
        );
        assert_eq!(
            (record.catalog.as_str(), record.email.as_str()),
            (*catalog, email.as_str()),
            "case {case:?}: the record names the catalog and the address the code was sent to"
        );
        assert_eq!(
            record.sends, 1,
            "case {case:?}: one code has been sent to this address, got {}",
            record.sends
        );
        assert_eq!(
            record.expires_at,
            record.sent_at + EMAIL_VERIFICATION_CODE_TTL,
            "case {case:?}: a code falls due one lifetime after it was sent"
        );
        assert!(
            !body.to_string().contains(&record.code),
            "case {case:?}: the code is sensitive and must not reach the response: {body}"
        );
        assert!(
            codes.insert(record.code.clone()),
            "case {case:?}: every address is sent its own code, got {:?} twice",
            record.code
        );
    }

    assert_eq!(
        held_count(&emulator),
        rows.len(),
        "each row's address holds its own code"
    );
}

/// A malformed address is refused with the modelled `ValidationException` at HTTP
/// 400 and nothing is stored for it. `Email`'s `@pattern` and `@length` are what
/// decide whether this service sends to an address, so the whole refusal class is
/// the ways an address can fall outside them, and implementing the operation must
/// not start serving any of them.
///
/// Rows: no `@` at all (the form the recorded call carried), an empty local part, an
/// empty domain, a second `@`, whitespace inside the address, a domain label on
/// either side of the dash rule (leading and trailing), an empty domain label, a
/// trailing dot, and `@length`'s maximum plus one — the accepted table above covers
/// the maximum itself.
#[tokio::test]
async fn a_malformed_address_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let over_long = format!("{}@example.com", "a".repeat(EMAIL_MAX_LEN - "@example.com".len() + 1));

    // (case, address)
    let rows: Vec<(&str, String)> = vec![
        ("no @ at all", "not-an-email-address".to_string()),
        ("an empty local part", "@example.com".to_string()),
        ("an empty domain", "partner@".to_string()),
        ("a second @", "two@@example.com".to_string()),
        ("a space in the local part", "part ner@example.com".to_string()),
        ("a space in the domain", "partner@exa mple.com".to_string()),
        ("a domain label opening with a dash", "partner@-example.com".to_string()),
        ("a domain label closing with a dash", "partner@example-.com".to_string()),
        ("an empty domain label", "partner@example..com".to_string()),
        ("a trailing dot", "partner@example.com.".to_string()),
        ("one character past Email's maximum", over_long.clone()),
    ];

    for (case, email) in &rows {
        let answer = emulator
            .call("SendEmailVerificationCode", &send("AWS", email))
            .await;
        assert_error(case, answer, 400, "ValidationException");
        assert!(
            held(&emulator, "AWS", email).is_none(),
            "case {case:?}: a refused send stores no code"
        );
    }

    assert_eq!(
        held_count(&emulator),
        0,
        "every row was refused, so no address holds a code"
    );
}

/// Sending again to an address REPLACES the code it holds rather than adding a
/// second one, and leaves every other address's code alone. A code the clock has
/// made due is no longer held, so the next send to that address is a first send.
#[tokio::test]
async fn a_resend_replaces_the_code_an_address_holds() {
    let emulator = Emulator::new();
    let other = "other@example.com";
    emulator
        .ok("SendEmailVerificationCode", &send("AWS", other))
        .await;
    let untouched = held(&emulator, "AWS", other).expect("the other address holds a code");

    let email = "partner@example.net";
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for send_number in 1..=3u64 {
        emulator
            .ok("SendEmailVerificationCode", &send("AWS", email))
            .await;
        let record = held(&emulator, "AWS", email).expect("the address holds a code");
        assert_eq!(
            record.sends, send_number,
            "send {send_number}: the record counts the codes sent to this address, got {}",
            record.sends
        );
        assert!(
            seen.insert(record.code.clone()),
            "send {send_number}: a resend sends a new code, got {:?} again",
            record.code
        );
    }
    assert_eq!(
        held_count(&emulator),
        2,
        "a resend replaces the code an address holds rather than storing another"
    );
    assert_eq!(
        held(&emulator, "AWS", other).map(|record| record.code),
        Some(untouched.code),
        "a send to one address leaves another address's code alone"
    );

    // The same address in another catalog is another address: codes are held per
    // (catalog, email).
    emulator
        .ok("SendEmailVerificationCode", &send("Sandbox", email))
        .await;
    assert_ne!(
        held(&emulator, "Sandbox", email).map(|record| record.code),
        held(&emulator, "AWS", email).map(|record| record.code),
        "each catalog holds its own code for an address"
    );
    assert_eq!(
        held_count(&emulator),
        3,
        "the catalogs hold their codes side by side"
    );
}

/// A code whose lifetime has passed is no longer held — the removal the clock owed
/// at that instant, materialised whether or not the next caller asked about codes —
/// and the next send to that address is a first send again.
#[tokio::test]
async fn a_code_past_its_lifetime_is_no_longer_held() {
    let emulator = Emulator::new();
    let email = "expiring@example.com";
    emulator
        .ok("SendEmailVerificationCode", &send("AWS", email))
        .await;
    let sent = held(&emulator, "AWS", email).expect("the address holds a code");

    // There is no other way to reach an instant one code lifetime out.
    emulator
        .engine
        .state::<PartnerCentralState>()
        .lock()
        .email_verification_codes
        .get_mut(&("AWS".to_string(), email.to_string()))
        .expect("the code this test sent is stored")
        .expires_at = sent.sent_at - EMAIL_VERIFICATION_CODE_TTL;

    // Any later call materialises what the clock already made due.
    emulator
        .ok("ListConnectionInvitations", &json!({ "Catalog": "AWS" }))
        .await;
    assert!(
        held(&emulator, "AWS", email).is_none(),
        "a code past its lifetime is no longer held"
    );

    emulator
        .ok("SendEmailVerificationCode", &send("AWS", email))
        .await;
    let resent = held(&emulator, "AWS", email).expect("the address holds a code again");
    assert_eq!(
        resent.sends, 1,
        "the address holds no code, so this is a first send, got {}",
        resent.sends
    );
    assert_ne!(
        resent.code, sent.code,
        "the code sent after the lifetime passed is a new one"
    );
}

/// Past the ceiling on the addresses this emulator holds a code for, a send to a NEW
/// address is refused with the modelled `ServiceQuotaExceededException` (HTTP 402)
/// and its email reason, while a resend to an address already holding one still
/// works: a resend replaces, so only a first send grows the store.
#[tokio::test]
async fn a_send_past_the_email_ceiling_is_refused_with_the_quota_error() {
    let emulator = Emulator::new();

    for index in 0..MAX_VERIFICATION_EMAILS {
        let email = format!("partner-{index}@example.com");
        let (status, body) = emulator
            .call("SendEmailVerificationCode", &send("AWS", &email))
            .await;
        assert_eq!(
            status, 200,
            "address {index}: a send inside the ceiling is served, got {status}: {body}"
        );
    }
    assert_eq!(
        held_count(&emulator),
        MAX_VERIFICATION_EMAILS,
        "the ceiling's worth of addresses hold a code"
    );

    let over = "one-too-many@example.com";
    let (status, body) = emulator
        .call("SendEmailVerificationCode", &send("AWS", over))
        .await;
    assert_eq!(
        status, 402,
        "a send past the ceiling is refused, got {status}: {body}"
    );
    let code = error_code(&body);
    assert!(
        code.contains("ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException, got {code:?}: {body}"
    );
    assert_eq!(
        body.get("Reason").and_then(Value::as_str),
        Some("LIMIT_EXCEEDED_NUMBER_OF_EMAIL"),
        "the refusal names the quota it would exceed: {body}"
    );
    assert!(
        string(&body, "Message", "the email ceiling")
            .contains(&MAX_VERIFICATION_EMAILS.to_string()),
        "the message states the ceiling the send would exceed: {body}"
    );
    assert!(
        held(&emulator, "AWS", over).is_none(),
        "a refused send stores no code"
    );

    // A resend replaces a code, so it is served at the ceiling.
    emulator
        .ok(
            "SendEmailVerificationCode",
            &send("AWS", "partner-0@example.com"),
        )
        .await;
    assert_eq!(
        held_count(&emulator),
        MAX_VERIFICATION_EMAILS,
        "a resend at the ceiling replaces a code rather than storing another"
    );
}
