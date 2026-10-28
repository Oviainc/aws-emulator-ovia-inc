// SPDX-License-Identifier: CC0-1.0

//! Easy DKIM for an identity, end to end over the wire.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind
//! them all. What is asserted is the RULE the three operations follow over a SET
//! of inputs, not one recorded request:
//!
//! * `VerifyDomainDkim` adds the domain to the identity list, generates its Easy
//!   DKIM tokens ONCE and stores them on that identity, so asking again answers
//!   with the same tokens and `GetIdentityDkimAttributes` reports exactly what was
//!   stored — for every form of domain name the service accepts, and with no two
//!   identities sharing a token;
//! * a generated setup is reported `Pending` — Amazon SES has yet to read the
//!   CNAME records the caller was told to publish — and reading it is not what
//!   moves it, so every later reader sees what the one before it saw;
//! * the setup REPORTED for an identity is the one that covers it: its own for a
//!   domain, its domain's for an email address, and none at all — `NotStarted`,
//!   with no tokens — for an identity nothing covers;
//! * `SetIdentityDkimEnabled` stores the signing flag on the named identity's own
//!   record, so one identity's flag says nothing about another's, and enabling an
//!   identity no Easy DKIM setup covers is refused with HTTP 400;
//! * a value that is no domain name is refused with HTTP 400 and nothing is
//!   stored; only identities in the list are reported, and a report may name at
//!   most 100 of them;
//! * the report is a MAP of the requested identities the account holds, each
//!   entry resolved from that identity's OWN record and the setup covering it, so
//!   one call naming many identities reports every one of them independently, an
//!   identity named more than once is reported once, and the order they were
//!   named in changes nothing.

use crate::harness::{call, element, elements, emulator, field, map_entries, Field};
use service_com_amazonaws_ses_simpleemailservice::ops::VERIFICATION_STATUS_VALUES;
use service_com_amazonaws_ses_simpleemailservice::state::{
    DKIM_SIGNING_AT_IDENTITY_START, EASY_DKIM_TOKEN_COUNT, EASY_DKIM_TOKEN_LEN,
    MAX_IDENTITIES_PER_CALL,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `VerificationStatus` an Easy DKIM setup whose records are unpublished
/// carries.
const PENDING: &str = "Pending";

/// The `VerificationStatus` reported for an identity no Easy DKIM setup covers.
const NOT_STARTED: &str = "NotStarted";

/// The Region the ARNs in this suite name — the one the request's SigV4 scope
/// carries (`crate::harness::AUTHORIZATION`).
const REGION: &str = "us-east-1";

/// What `GetIdentityDkimAttributes` reports for one identity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// `DkimEnabled`, as the wire spells the boolean.
    enabled: String,
    /// `DkimVerificationStatus`.
    status: String,
    /// `DkimTokens`, and `None` when the report carries no such member at all.
    tokens: Option<Vec<String>>,
}

/// `VerifyDomainDkim`, as `(status, body)`.
async fn verify_domain_dkim(router: &MultiServiceRouter, domain: &str) -> (u16, String) {
    call(router, "VerifyDomainDkim", &[field("Domain", domain)]).await
}

/// `VerifyDomainDkim`, asserting it was accepted and answering with the tokens it
/// handed out.
async fn verify_domain_dkim_ok(router: &MultiServiceRouter, domain: &str) -> Vec<String> {
    let (status, body) = verify_domain_dkim(router, domain).await;
    assert_eq!(
        status, 200,
        "case VerifyDomainDkim({domain:?}): expected 200, got {status} with {body}"
    );
    let tokens = element(&body, "DkimTokens")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    assert_eq!(
        tokens.len(),
        EASY_DKIM_TOKEN_COUNT,
        "case VerifyDomainDkim({domain:?}): expected {EASY_DKIM_TOKEN_COUNT} tokens, got \
         {tokens:?}"
    );
    tokens
}

/// `VerifyDomainIdentity`, which puts a domain in the identity list WITHOUT
/// setting Easy DKIM up for it.
async fn verify_domain_identity(router: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "VerifyDomainIdentity({domain:?}): expected 200, got {status} with {body}"
    );
}

/// `VerifyEmailIdentity`, the same for an address.
async fn verify_email_identity(router: &MultiServiceRouter, address: &str) {
    let (status, body) = call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await;
    assert_eq!(
        status, 200,
        "VerifyEmailIdentity({address:?}): expected 200, got {status} with {body}"
    );
}

/// `SetIdentityDkimEnabled`, as `(status, body)`.
async fn set_dkim_enabled(
    router: &MultiServiceRouter,
    identity: &str,
    enabled: bool,
) -> (u16, String) {
    call(
        router,
        "SetIdentityDkimEnabled",
        &[
            field("Identity", identity),
            field("DkimEnabled", enabled.to_string()),
        ],
    )
    .await
}

/// `SetIdentityDkimEnabled`, asserting it was accepted.
async fn set_dkim_enabled_ok(router: &MultiServiceRouter, identity: &str, enabled: bool) {
    let (status, body) = set_dkim_enabled(router, identity, enabled).await;
    assert_eq!(
        status, 200,
        "case SetIdentityDkimEnabled({identity:?}, {enabled}): expected 200, got {status} with \
         {body}"
    );
}

/// `GetIdentityDkimAttributes` over `identities`, as `(status, body)`.
async fn dkim_attributes(router: &MultiServiceRouter, identities: &[&str]) -> (u16, String) {
    let params: Vec<Field> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| field(format!("Identities.member.{}", index + 1), *identity))
        .collect();
    call(router, "GetIdentityDkimAttributes", &params).await
}

/// Every identity `GetIdentityDkimAttributes` reports for `identities`, with what
/// it reported for each.
async fn reported_all(router: &MultiServiceRouter, identities: &[&str]) -> Vec<(String, Reported)> {
    let (status, body) = dkim_attributes(router, identities).await;
    assert_eq!(
        status, 200,
        "GetIdentityDkimAttributes({identities:?}): expected 200, got {status} with {body}"
    );
    map_entries(&body)
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                Reported {
                    enabled: element(&value, "DkimEnabled").unwrap_or_default(),
                    status: element(&value, "DkimVerificationStatus").unwrap_or_default(),
                    tokens: element(&value, "DkimTokens")
                        .map(|list| elements(&list, "member")),
                },
            )
        })
        .collect()
}

/// What `GetIdentityDkimAttributes` reports for ONE identity, and `None` when the
/// report does not carry it at all.
async fn reported(router: &MultiServiceRouter, identity: &str) -> Option<Reported> {
    reported_all(router, &[identity])
        .await
        .into_iter()
        .find(|(key, _)| key == identity)
        .map(|(_, attributes)| attributes)
}

/// Whether `token` has the shape of an Easy DKIM token: the published length, and
/// the lowercase alphanumeric characters a DNS label may carry.
fn is_token_shaped(token: &str) -> bool {
    token.len() == EASY_DKIM_TOKEN_LEN
        && token
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

#[tokio::test]
async fn every_status_this_suite_expects_is_one_the_model_declares() {
    // The suite's expectations are pinned against the model's own published value
    // set, so a model that renamed one of them fails here rather than letting a
    // report carry a status SES never answers with.
    for status in [PENDING, NOT_STARTED] {
        assert!(
            VERIFICATION_STATUS_VALUES.contains(&status),
            "case {status:?}: expected one of {VERIFICATION_STATUS_VALUES:?}"
        );
    }
}

#[tokio::test]
async fn easy_dkim_tokens_are_generated_once_per_domain_and_reported_back() {
    let router = emulator();
    // The rows differ along what the rule generalises over: the number of labels,
    // the characters a label carries, the top-level domain (including an
    // internationalised one), letter case, and the fully qualified form with its
    // root label. `expected_key` is the identity the domain is stored as.
    let cases: &[(&str, &str)] = &[
        ("example.com", "example.com"),
        ("mail.example.com", "mail.example.com"),
        ("a.b.c.example.net", "a.b.c.example.net"),
        ("ex-ample.co.uk", "ex-ample.co.uk"),
        ("d1g1ts.example.io", "d1g1ts.example.io"),
        // An internationalised top-level domain in its `xn--` form, which
        // `is_domain_name` admits.
        ("punycode.xn--kprw13d", "punycode.xn--kprw13d"),
        ("LOUD.example.ORG", "LOUD.example.ORG"),
        // Fully qualified: the root label names the same domain, so the identity
        // is stored without it.
        ("trailing.example.com.", "trailing.example.com"),
    ];

    let mut every_token: Vec<(String, String)> = Vec::new();
    for (domain, expected_key) in cases {
        let tokens = verify_domain_dkim_ok(&router, domain).await;
        for token in &tokens {
            assert!(
                is_token_shaped(token),
                "case {domain:?}: expected a {EASY_DKIM_TOKEN_LEN}-character lowercase \
                 alphanumeric token, got {token:?}"
            );
            if let Some((owner, _)) = every_token.iter().find(|(_, held)| held == token) {
                panic!("case {domain:?}: token {token:?} was already handed to {owner:?}");
            }
            every_token.push(((*domain).to_string(), token.clone()));
        }

        // Asking again hands back the tokens the domain already carries: the CNAME
        // records a caller has published must keep matching what SES reports.
        let again = verify_domain_dkim_ok(&router, domain).await;
        assert_eq!(
            again, tokens,
            "case {domain:?}: a repeat VerifyDomainDkim must not generate new tokens, got \
             {again:?}"
        );

        // And the report is the STORED setup, under the key the domain was stored
        // as.
        let reported = reported(&router, expected_key).await;
        assert_eq!(
            reported,
            Some(Reported {
                enabled: DKIM_SIGNING_AT_IDENTITY_START.to_string(),
                status: PENDING.to_string(),
                tokens: Some(tokens.clone()),
            }),
            "case {domain:?}: expected the stored tokens {tokens:?} reported as {PENDING}, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn verify_domain_dkim_adds_a_domain_the_account_had_not_verified() {
    // "The domain that you specify is added to the list of identities that are
    // associated with your account. This is true even if you haven't already
    // associated the domain with your account by using the VerifyDomainIdentity
    // operation."
    let router = emulator();
    let domain = "never-verified.example.com";

    let (status, body) = call(
        &router,
        "GetIdentityVerificationAttributes",
        &[field("Identities.member.1", domain)],
    )
    .await;
    assert_eq!(
        status, 200,
        "GetIdentityVerificationAttributes before the call: expected 200, got {status} with {body}"
    );
    assert!(
        map_entries(&body).is_empty(),
        "case before VerifyDomainDkim: expected {domain:?} to be in no identity list, got {body}"
    );

    verify_domain_dkim_ok(&router, domain).await;

    let (status, body) = call(
        &router,
        "GetIdentityVerificationAttributes",
        &[field("Identities.member.1", domain)],
    )
    .await;
    assert_eq!(
        status, 200,
        "GetIdentityVerificationAttributes after the call: expected 200, got {status} with {body}"
    );
    let entries = map_entries(&body);
    let verification = entries
        .iter()
        .find(|(key, _)| key == domain)
        .map(|(_, value)| value.clone());
    let Some(verification) = verification else {
        panic!("case after VerifyDomainDkim: expected {domain:?} in the identity list, got {body}");
    };
    assert_eq!(
        element(&verification, "VerificationStatus"),
        Some(PENDING.to_string()),
        "case after VerifyDomainDkim: expected the new domain identity {PENDING}, got \
         {verification}"
    );
    // A domain identity carries a TXT verification token of its own, whichever
    // operation added it.
    let token = element(&verification, "VerificationToken").unwrap_or_default();
    assert!(
        !token.is_empty(),
        "case after VerifyDomainDkim: expected a domain verification token, got {verification}"
    );
}

#[tokio::test]
async fn the_setup_reported_for_an_identity_is_the_one_that_covers_it() {
    // "A set of DKIM tokens that represent the identity. If the identity is an
    // email address, the tokens represent the domain of that address."
    let router = emulator();
    let set_up = "covered.example.com";
    let tokens = verify_domain_dkim_ok(&router, set_up).await;

    // An address under the set-up domain, and one under a subdomain of it, are
    // both covered by it; a domain identity is not covered by the domain above it,
    // which needs tokens of its own.
    verify_email_identity(&router, "user@covered.example.com").await;
    verify_email_identity(&router, "user@mail.covered.example.com").await;
    verify_domain_identity(&router, "mail.covered.example.com").await;
    // And an identity whose domain has no setup at all is covered by nothing.
    verify_email_identity(&router, "user@bare.example.net").await;
    verify_domain_identity(&router, "bare.example.net").await;

    let covered: &[&str] = &[
        set_up,
        "user@covered.example.com",
        "user@mail.covered.example.com",
    ];
    for identity in covered {
        let reported = reported(&router, identity).await;
        assert_eq!(
            reported,
            Some(Reported {
                enabled: DKIM_SIGNING_AT_IDENTITY_START.to_string(),
                status: PENDING.to_string(),
                tokens: Some(tokens.clone()),
            }),
            "case {identity:?}: expected the covering setup's tokens {tokens:?}, got {reported:?}"
        );
    }

    let uncovered: &[&str] = &[
        "mail.covered.example.com",
        "user@bare.example.net",
        "bare.example.net",
    ];
    for identity in uncovered {
        let reported = reported(&router, identity).await;
        assert_eq!(
            reported,
            Some(Reported {
                enabled: DKIM_SIGNING_AT_IDENTITY_START.to_string(),
                status: NOT_STARTED.to_string(),
                tokens: None,
            }),
            "case {identity:?}: expected {NOT_STARTED} with no tokens, got {reported:?}"
        );
    }

    // An identity this account has not verified is ABSENT from the report, so a
    // caller can tell "no Easy DKIM" from "not known to this account".
    let stranger = "stranger.example.org";
    let listed = reported_all(&router, &[stranger, set_up]).await;
    assert_eq!(
        listed.iter().filter(|(key, _)| key == stranger).count(),
        0,
        "case {stranger:?}: an unverified identity must not be reported, got {listed:?}"
    );
    assert_eq!(
        listed.iter().filter(|(key, _)| key == set_up).count(),
        1,
        "case {set_up:?}: expected one entry for the verified identity, got {listed:?}"
    );
}

#[tokio::test]
async fn reporting_a_setup_is_not_what_moves_it() {
    // Amazon SES reports the setup `Pending` until it has read the CNAME records
    // the caller was told to publish. Nothing in this API publishes them, and
    // reading the attributes is not publishing them, so every reader sees the
    // same `Pending` — which is what a domain whose tokens were never published
    // reads as at Amazon SES too.
    let router = emulator();
    let domain = "pending.example.com";
    let address = "user@pending.example.com";
    let tokens = verify_domain_dkim_ok(&router, domain).await;
    verify_email_identity(&router, address).await;

    let expected = Some(Reported {
        enabled: DKIM_SIGNING_AT_IDENTITY_START.to_string(),
        status: PENDING.to_string(),
        tokens: Some(tokens.clone()),
    });
    for read in 1..=3 {
        let report = reported(&router, domain).await;
        assert_eq!(
            report, expected,
            "case read {read}: expected {expected:?}, got {report:?}"
        );
        // The address the setup covers reads the same record, so it reports the
        // same thing.
        let inherited = reported(&router, address).await;
        assert_eq!(
            inherited, expected,
            "case the covered address, read {read}: expected {expected:?}, got {inherited:?}"
        );
    }

    // And asking for Easy DKIM again changes nothing a later reader sees.
    let repeat = verify_domain_dkim_ok(&router, domain).await;
    assert_eq!(
        repeat, tokens,
        "case a repeat call: expected the stored tokens {tokens:?}, got {repeat:?}"
    );
    let unmoved = reported(&router, domain).await;
    assert_eq!(
        unmoved, expected,
        "case a repeat call: expected {expected:?}, got {unmoved:?}"
    );
}

#[tokio::test]
async fn dkim_signing_is_stored_on_the_named_identity_alone() {
    let router = emulator();
    let first = "signing.example.com";
    let second = "other-signing.example.com";
    verify_domain_dkim_ok(&router, first).await;
    verify_domain_dkim_ok(&router, second).await;

    // Both forms the `Identity` member documents name the same identity: the name
    // itself, and an ARN of this service naming it.
    let named_both_ways: &[&str] = &[
        first,
        &format!("arn:aws:ses:{REGION}:000000000000:identity/{first}"),
    ];
    for identity in named_both_ways {
        for enabled in [false, true] {
            set_dkim_enabled_ok(&router, identity, enabled).await;
            let report = reported(&router, first).await;
            assert_eq!(
                report.as_ref().map(|report| report.enabled.clone()),
                Some(enabled.to_string()),
                "case SetIdentityDkimEnabled({identity:?}, {enabled}): expected DkimEnabled \
                 {enabled}, got {report:?}"
            );
            // One identity's flag says nothing about another's.
            let untouched = reported(&router, second).await;
            assert_eq!(
                untouched.as_ref().map(|report| report.enabled.clone()),
                Some(DKIM_SIGNING_AT_IDENTITY_START.to_string()),
                "case SetIdentityDkimEnabled({identity:?}, {enabled}): expected {second:?} to \
                 keep its own flag, got {untouched:?}"
            );
        }
    }
}

#[tokio::test]
async fn enabling_dkim_signing_without_a_setup_is_refused_and_the_flag_stands() {
    // "For email addresses (for example, user@example.com), you can only enable
    // DKIM signing if the corresponding domain (in this case, example.com) has
    // been set up to use Easy DKIM."
    let router = emulator();
    verify_domain_identity(&router, "no-dkim.example.com").await;
    verify_email_identity(&router, "user@no-dkim.example.com").await;

    let cases: &[&str] = &["no-dkim.example.com", "user@no-dkim.example.com"];
    for identity in cases {
        // Disabling is always allowed: there is nothing to sign with either way.
        set_dkim_enabled_ok(&router, identity, false).await;
        let (status, body) = set_dkim_enabled(&router, identity, true).await;
        assert_eq!(
            status, 400,
            "case SetIdentityDkimEnabled({identity:?}, true) with no setup: expected 400, got \
             {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code"),
            Some("InvalidParameterValue".to_string()),
            "case SetIdentityDkimEnabled({identity:?}, true) with no setup: expected \
             InvalidParameterValue, got {body}"
        );
        let report = reported(&router, identity).await;
        assert_eq!(
            report.as_ref().map(|report| report.enabled.clone()),
            Some(false.to_string()),
            "case SetIdentityDkimEnabled({identity:?}, true) with no setup: a refused request \
             must leave the flag as it was, got {report:?}"
        );
    }

    // An identity this account does not hold is refused too, and nothing is
    // stored for it.
    let stranger = "stranger.example.net";
    let (status, body) = set_dkim_enabled(&router, stranger, false).await;
    assert_eq!(
        status, 400,
        "case SetIdentityDkimEnabled({stranger:?}): expected 400, got {status} with {body}"
    );
    assert!(
        reported(&router, stranger).await.is_none(),
        "case SetIdentityDkimEnabled({stranger:?}): a refused request must store nothing"
    );
}

#[tokio::test]
async fn a_domain_that_is_no_domain_name_is_refused_and_nothing_is_stored() {
    let router = emulator();
    // The rows span the ways a value fails the `[SubDomain.]Domain.TopLevelDomain`
    // form: no label separator, an empty label, whitespace inside, a character no
    // label admits, an all-numeric top-level label, and nothing at all.
    let cases: &[&str] = &[
        "example",
        "example..com",
        "exam ple.com",
        "under_score.example.com",
        "example.123",
        "user@example.com",
        "",
    ];
    for domain in cases {
        let (status, body) = verify_domain_dkim(&router, domain).await;
        assert_eq!(
            status, 400,
            "case VerifyDomainDkim({domain:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code"),
            Some("ValidationException".to_string()),
            "case VerifyDomainDkim({domain:?}): expected ValidationException, got {body}"
        );
        assert!(
            reported(&router, domain).await.is_none(),
            "case VerifyDomainDkim({domain:?}): a refused request must store no identity"
        );
    }
}

#[tokio::test]
async fn a_report_may_name_at_most_the_published_number_of_identities() {
    let router = emulator();
    let identities: Vec<String> = (0..=MAX_IDENTITIES_PER_CALL)
        .map(|index| format!("bulk{index}.example.com"))
        .collect();

    let at_the_bound: Vec<&str> = identities
        .iter()
        .take(MAX_IDENTITIES_PER_CALL)
        .map(String::as_str)
        .collect();
    let (status, body) = dkim_attributes(&router, &at_the_bound).await;
    assert_eq!(
        status, 200,
        "case {MAX_IDENTITIES_PER_CALL} identities: expected 200, got {status} with {body}"
    );

    let past_the_bound: Vec<&str> = identities.iter().map(String::as_str).collect();
    let (status, body) = dkim_attributes(&router, &past_the_bound).await;
    assert_eq!(
        status,
        400,
        "case {} identities: expected 400, got {status} with {body}",
        past_the_bound.len()
    );
    assert_eq!(
        element(&body, "Code"),
        Some("InvalidParameterValue".to_string()),
        "case {} identities: expected InvalidParameterValue, got {body}",
        past_the_bound.len()
    );
}

/// One identity of the account, and what the report must carry for it — resolved
/// from that identity's own record and the Easy DKIM setup that covers it.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The identity, as it was recorded and as the request names it.
    identity: &'static str,
    /// The entry the report must carry for it.
    expected: Reported,
}

#[tokio::test]
async fn one_call_reports_every_requested_identity_from_its_own_record() {
    let router = emulator();

    // Two Easy DKIM setups, so an identity reported with one cannot be passing by
    // carrying the other's tokens, and a domain identity with no setup at all.
    let signed = "signed.example.com";
    let signed_tokens = verify_domain_dkim_ok(&router, signed).await;
    let other = "other-signed.example.net";
    let other_tokens = verify_domain_dkim_ok(&router, other).await;
    let plain = "plain.example.org";
    verify_domain_identity(&router, plain).await;

    // Addresses under each setup — one of them a label deeper — and one nothing
    // covers.
    for address in [
        "user@signed.example.com",
        "off@signed.example.com",
        "user@mail.signed.example.com",
        "user@other-signed.example.net",
        "user@plain.example.org",
    ] {
        verify_email_identity(&router, address).await;
    }

    // Signing is stored on the identity it was set on, so switching it off on a
    // domain and on one address says nothing about the identities beside them —
    // including the ones that share their covering setup.
    set_dkim_enabled_ok(&router, other, false).await;
    set_dkim_enabled_ok(&router, "off@signed.example.com", false).await;

    // `IdentityDkimAttributes$DkimEnabled` is documented "Is true if DKIM signing
    // is enabled for email sent from the identity. It's false otherwise. The
    // default value is true", so an identity nothing switched off reports `true` —
    // spelled out here rather than read from the implementation's own constant.
    let on = true.to_string();
    let off = false.to_string();
    // The rows span what one entry is resolved from: the class of identity
    // (domain, address), whether a setup covers it and how far above it that
    // setup sits, and that identity's own signing flag.
    let rows = vec![
        Row {
            case: "a domain with its own Easy DKIM setup",
            identity: signed,
            expected: Reported {
                enabled: on.clone(),
                status: PENDING.to_string(),
                tokens: Some(signed_tokens.clone()),
            },
        },
        Row {
            case: "a domain with its own setup and signing off",
            identity: other,
            expected: Reported {
                enabled: off.clone(),
                status: PENDING.to_string(),
                tokens: Some(other_tokens.clone()),
            },
        },
        Row {
            case: "a domain no Easy DKIM setup covers",
            identity: plain,
            expected: Reported {
                enabled: on.clone(),
                status: NOT_STARTED.to_string(),
                tokens: None,
            },
        },
        Row {
            case: "an address under the first setup",
            identity: "user@signed.example.com",
            expected: Reported {
                enabled: on.clone(),
                status: PENDING.to_string(),
                tokens: Some(signed_tokens.clone()),
            },
        },
        Row {
            case: "an address under the first setup with signing off",
            identity: "off@signed.example.com",
            expected: Reported {
                enabled: off,
                status: PENDING.to_string(),
                tokens: Some(signed_tokens.clone()),
            },
        },
        Row {
            case: "an address under a subdomain of the first setup",
            identity: "user@mail.signed.example.com",
            expected: Reported {
                enabled: on.clone(),
                status: PENDING.to_string(),
                tokens: Some(signed_tokens),
            },
        },
        Row {
            case: "an address under the second setup, whose domain has signing off",
            identity: "user@other-signed.example.net",
            expected: Reported {
                enabled: on.clone(),
                status: PENDING.to_string(),
                tokens: Some(other_tokens),
            },
        },
        Row {
            case: "an address no Easy DKIM setup covers",
            identity: "user@plain.example.org",
            expected: Reported {
                enabled: on,
                status: NOT_STARTED.to_string(),
                tokens: None,
            },
        },
    ];

    // Named in one call, together with an identity the account does not hold.
    let stranger = "stranger.example.com";
    let mut named: Vec<&str> = rows.iter().map(|row| row.identity).collect();
    named.push(stranger);

    // The report is a map, so the order the identities were named in is not part
    // of the question asked.
    for ordering in ["as named", "in the reverse order"] {
        let mut requested = named.clone();
        if ordering == "in the reverse order" {
            requested.reverse();
        }
        let listed = reported_all(&router, &requested).await;
        assert_eq!(
            listed.len(),
            rows.len(),
            "case {ordering}: expected one entry per identity the account holds ({}), got \
             {listed:?}",
            rows.len()
        );
        for row in &rows {
            let found = listed
                .iter()
                .find(|(key, _)| key == row.identity)
                .map(|(_, attributes)| attributes.clone());
            assert_eq!(
                found,
                Some(row.expected.clone()),
                "case {:?} ({ordering}): expected {:?}, got {found:?}",
                row.case,
                row.expected
            );
        }
        assert!(
            !listed.iter().any(|(key, _)| key == stranger),
            "case {ordering}: an unverified identity must not be reported, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn naming_an_identity_more_than_once_reports_it_once() {
    // The report is a map keyed by identity, so a request that names one twice
    // asks about one identity.
    let router = emulator();
    let domain = "repeated.example.com";
    let tokens = verify_domain_dkim_ok(&router, domain).await;
    let address = "user@repeated.example.com";
    verify_email_identity(&router, address).await;
    let stranger = "stranger.example.net";

    // Both identities are covered by the one setup and keep the documented default
    // signing flag (`IdentityDkimAttributes$DkimEnabled`: "The default value is
    // true"), so every entry any of these rows reports is this.
    let expected = Reported {
        enabled: true.to_string(),
        status: PENDING.to_string(),
        tokens: Some(tokens),
    };

    // The rows differ in how the request repeats an identity: twice, three times,
    // beside another identity, and around one the account does not hold.
    let cases: &[(&str, &[&str], usize)] = &[
        ("one identity named twice", &[domain, domain], 1),
        ("one identity named three times", &[domain, domain, domain], 1),
        ("two identities, one of them twice", &[domain, address, domain], 2),
        (
            "a repeat around an identity the account does not hold",
            &[domain, stranger, domain],
            1,
        ),
    ];
    for (case, requested, entries) in cases {
        let listed = reported_all(&router, requested).await;
        assert_eq!(
            listed.len(),
            *entries,
            "case {case:?}: expected {entries} entry/entries, got {listed:?}"
        );
        for (key, reported) in &listed {
            assert_eq!(
                reported, &expected,
                "case {case:?}: expected {key:?} reported as {expected:?}, got {reported:?}"
            );
        }
    }
}
