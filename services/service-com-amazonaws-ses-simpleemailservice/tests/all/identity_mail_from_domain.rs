// SPDX-License-Identifier: CC0-1.0

//! An identity's custom MAIL FROM setup, end to end over the wire.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind
//! them all. What is asserted is the RULE the two operations follow over a SET of
//! inputs, not one recorded request:
//!
//! * `SetIdentityMailFromDomain` stores the custom MAIL FROM domain and the
//!   MX-failure behaviour on the named identity's own record, and
//!   `GetIdentityMailFromDomainAttributes` reports exactly what was stored, for
//!   every behaviour the model declares and for a domain and an email-address
//!   identity alike;
//! * the domain has to be a subdomain of the verified identity — a domain outside
//!   it, the identity's own domain, a parent of it and a value that is no domain
//!   name at all are refused with HTTP 400 `InvalidParameterValue`, leaving the
//!   stored setup as it was;
//! * a recorded setup starts `Pending` and the clock owes its move to `Success`,
//!   so a caller reading it straight back sees `Pending` and one reading it after
//!   the span sees `Success` — and reporting it changes nothing a later reader
//!   sees;
//! * an omitted, empty or blank `MailFromDomain` disables the custom MAIL FROM
//!   setting for the identity;
//! * an identity this account has not verified is refused and nothing is stored;
//!   only identities in the list are reported, and a call may ask about at most
//!   100 of them;
//! * every member reported for an identity comes from that identity's own record,
//!   so one call reporting differently configured identities answers each of them
//!   from its own setup.

use crate::harness::{call, element, emulator, field, map_entries, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::ops::BEHAVIOR_ON_M_X_FAILURE_VALUES;
use service_com_amazonaws_ses_simpleemailservice::state::MAIL_FROM_SETUP_SPAN;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the ARNs in this suite name; nothing here is Region-specific.
const REGION: &str = "us-east-1";

/// `VerifyEmailIdentity`, which is what puts an address in the identity list.
async fn verify_email(router: &MultiServiceRouter, address: &str) {
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

/// `VerifyDomainIdentity`, the same for a domain.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "VerifyDomainIdentity({domain:?}): expected 200, got {status} with {body}"
    );
}

/// `SetIdentityMailFromDomain`, as `(status, body)`. `domain` is `None` when the
/// request leaves `MailFromDomain` out altogether, and `behavior` when it leaves
/// `BehaviorOnMXFailure` out.
async fn set_mail_from(
    router: &MultiServiceRouter,
    identity: &str,
    domain: Option<&str>,
    behavior: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = vec![field("Identity", identity)];
    if let Some(domain) = domain {
        params.push(field("MailFromDomain", domain));
    }
    if let Some(behavior) = behavior {
        params.push(field("BehaviorOnMXFailure", behavior));
    }
    call(router, "SetIdentityMailFromDomain", &params).await
}

/// `SetIdentityMailFromDomain`, asserting it was accepted.
async fn set_mail_from_ok(
    router: &MultiServiceRouter,
    identity: &str,
    domain: Option<&str>,
    behavior: Option<&str>,
) {
    let (status, body) = set_mail_from(router, identity, domain, behavior).await;
    assert_eq!(
        status, 200,
        "case SetIdentityMailFromDomain({identity:?}, {domain:?}, {behavior:?}): expected 200, \
         got {status} with {body}"
    );
}

/// `GetIdentityMailFromDomainAttributes`, as `(identity, the attributes element's
/// inner XML)` for every identity the report carries.
async fn mail_from_attributes(
    router: &MultiServiceRouter,
    identities: &[&str],
) -> Vec<(String, String)> {
    let params: Vec<Field> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| field(format!("Identities.member.{}", index + 1), *identity))
        .collect();
    let (status, body) = call(router, "GetIdentityMailFromDomainAttributes", &params).await;
    assert_eq!(
        status, 200,
        "GetIdentityMailFromDomainAttributes({identities:?}): expected 200, got {status} with \
         {body}"
    );
    map_entries(&body)
}

/// The three members reported for `identity` — the domain, its status and the
/// MX-failure behaviour — or `None` when the report does not carry the identity.
///
/// A member the codec wrote empty reads back as the empty string, which is what
/// "no custom MAIL FROM domain" looks like on the wire.
async fn reported(router: &MultiServiceRouter, identity: &str) -> Option<(String, String, String)> {
    mail_from_attributes(router, &[identity])
        .await
        .into_iter()
        .find(|(key, _)| key == identity)
        .map(|(_, attributes)| {
            (
                element(&attributes, "MailFromDomain").unwrap_or_default(),
                element(&attributes, "MailFromDomainStatus").unwrap_or_default(),
                element(&attributes, "BehaviorOnMXFailure").unwrap_or_default(),
            )
        })
}

/// [`reported`], panicking when the identity is not in the report at all.
async fn reported_for(router: &MultiServiceRouter, identity: &str) -> (String, String, String) {
    reported(router, identity)
        .await
        .unwrap_or_else(|| panic!("{identity:?} must be reported"))
}

/// The identity ARN form the `Identity` member documents:
/// `arn:aws:ses:<region>:<account>:identity/<name>`.
fn identity_arn(name: &str) -> String {
    engine::emulator_arn("ses", REGION, &format!("identity/{name}"))
}

/// What a setup reports before the clock has moved it on: the status every fresh
/// setup starts in.
const PENDING: &str = "Pending";

/// What a setup reports once SES has read the MX record.
const SUCCESS: &str = "Success";

/// The behaviour a setup recorded without one carries, and the one an identity
/// with no custom MAIL FROM domain reports.
const USE_DEFAULT_VALUE: &str = "UseDefaultValue";

#[tokio::test]
async fn a_custom_mail_from_domain_is_stored_per_identity_and_read_back() {
    /// One identity, the MAIL FROM domain it is configured with and the
    /// behaviour the request names.
    struct Case {
        label: &'static str,
        /// The identity as it is verified, which is the key it is reported under.
        name: &'static str,
        /// A domain identity rather than an email address.
        domain_identity: bool,
        /// The `MailFromDomain` the request carries.
        requested: &'static str,
        /// The `BehaviorOnMXFailure` it carries, or `None` when it carries none.
        behavior: Option<&'static str>,
        /// The domain the report has to carry back.
        stored: &'static str,
        /// The behaviour it has to report.
        reported_behavior: &'static str,
    }

    // The rule generalises over the class of identity, how deep under it the MAIL
    // FROM domain sits, the behaviour named (every value
    // `BEHAVIOR_ON_M_X_FAILURE_VALUES` publishes, and none at all), the case the
    // domain is spelled in and whether it is fully qualified.
    let cases = [
        Case {
            label: "a domain identity, one label deeper, no behaviour named",
            name: "example.com",
            domain_identity: true,
            requested: "mail.example.com",
            behavior: None,
            stored: "mail.example.com",
            reported_behavior: USE_DEFAULT_VALUE,
        },
        Case {
            label: "a domain identity, two labels deeper, rejecting on MX failure",
            name: "corp.example.org",
            domain_identity: true,
            requested: "bounce.mail.corp.example.org",
            behavior: Some("RejectMessage"),
            stored: "bounce.mail.corp.example.org",
            reported_behavior: "RejectMessage",
        },
        Case {
            label: "a domain identity, the default behaviour named explicitly",
            name: "example.net",
            domain_identity: true,
            requested: "mail.example.net",
            behavior: Some("UseDefaultValue"),
            stored: "mail.example.net",
            reported_behavior: USE_DEFAULT_VALUE,
        },
        Case {
            label: "an email-address identity, a subdomain of the address's domain",
            name: "sender@mail.example.io",
            domain_identity: false,
            requested: "bounce.mail.example.io",
            behavior: Some("RejectMessage"),
            stored: "bounce.mail.example.io",
            reported_behavior: "RejectMessage",
        },
        Case {
            label: "a mixed-case domain under a lower-case identity",
            name: "example.co",
            domain_identity: true,
            requested: "Mail.Example.CO",
            behavior: None,
            // Stored as the request spelled it: a domain name is
            // case-insensitive, so the two spell the same domain.
            stored: "Mail.Example.CO",
            reported_behavior: USE_DEFAULT_VALUE,
        },
        Case {
            label: "a fully qualified domain, whose root label names the same domain",
            name: "example.dev",
            domain_identity: true,
            requested: "mail.example.dev.",
            behavior: None,
            stored: "mail.example.dev",
            reported_behavior: USE_DEFAULT_VALUE,
        },
    ];

    // Every behaviour the model publishes is exercised by the table above, so a
    // model that gained or renamed a value fails here rather than diverging
    // quietly.
    for value in BEHAVIOR_ON_M_X_FAILURE_VALUES {
        assert!(
            cases
                .iter()
                .any(|case| case.behavior == Some(*value) || case.reported_behavior == *value),
            "case BehaviorOnMXFailure={value:?}: the table must cover every published value"
        );
    }

    let router = emulator();
    for case in &cases {
        if case.domain_identity {
            verify_domain(&router, case.name).await;
        } else {
            verify_email(&router, case.name).await;
        }

        // A freshly verified identity has no custom MAIL FROM domain at all: no
        // domain and no setup status, and the behaviour that still delivers the
        // message.
        assert_eq!(
            reported_for(&router, case.name).await,
            (
                String::new(),
                String::new(),
                USE_DEFAULT_VALUE.to_string()
            ),
            "case {}: a fresh identity has no custom MAIL FROM setup",
            case.label
        );

        set_mail_from_ok(
            &router,
            case.name,
            Some(case.requested),
            case.behavior,
        )
        .await;
        let actual = reported_for(&router, case.name).await;
        assert_eq!(
            actual,
            (
                case.stored.to_string(),
                PENDING.to_string(),
                case.reported_behavior.to_string()
            ),
            "case {}: expected the stored setup, got {actual:?}",
            case.label
        );
    }

    // And each identity still reports its OWN setup when they are all asked about
    // in one call: nothing is shared between the records.
    let names: Vec<&str> = cases.iter().map(|case| case.name).collect();
    let report = mail_from_attributes(&router, &names).await;
    for case in &cases {
        let attributes = report
            .iter()
            .find(|(key, _)| key == case.name)
            .map(|(_, attributes)| attributes.clone())
            .unwrap_or_else(|| panic!("case {}: expected an entry", case.label));
        assert_eq!(
            (
                element(&attributes, "MailFromDomain").unwrap_or_default(),
                element(&attributes, "BehaviorOnMXFailure").unwrap_or_default()
            ),
            (case.stored.to_string(), case.reported_behavior.to_string()),
            "case {} in a shared report: expected its own setup, got {attributes:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_mail_from_domain_that_is_no_subdomain_of_the_identity_is_refused() {
    // `MailFromDomain`'s own documentation: "The MAIL FROM domain must 1) be a
    // subdomain of the verified identity". So a name outside the identity's
    // domain, the identity's domain itself, a parent of it, and a value that is
    // not a domain name at all are all refused.
    let router = emulator();
    let identity = "corp.example.com";
    verify_domain(&router, identity).await;
    let email_identity = "sender@corp.example.com";
    verify_email(&router, email_identity).await;

    let stored = "mail.corp.example.com";
    set_mail_from_ok(&router, identity, Some(stored), Some("RejectMessage")).await;

    // Each row is refused either because it is no subdomain of the identity or
    // because it is no domain name at all, and each refusal says which.
    let refused = [
        ("an unrelated domain", "mail.example.org", true),
        ("the identity's own domain", "corp.example.com", true),
        ("a parent of the identity", "example.com", true),
        ("a sibling sharing the suffix", "mail.notcorp.example.com", true),
        ("a name the identity is a suffix of", "corp.example.com.au", true),
        ("a single label", "localhost", false),
        ("a name with an empty label", "mail..corp.example.com", false),
        ("an address rather than a domain", "bounce@corp.example.com", false),
        ("a name with a space", "mail .corp.example.com", false),
    ];

    for (label, value, no_subdomain) in &refused {
        for named in [identity, email_identity] {
            let (status, body) = set_mail_from(&router, named, Some(value), None).await;
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("InvalidParameterValue")),
                "case {label} ({value:?}) on {named:?}: expected 400 InvalidParameterValue, got \
                 {status} with {body}"
            );
            let message = element(&body, "Message").map(|text| xml_text(&text));
            let expected = if *no_subdomain {
                format!(
                    "Provided MAIL-FROM domain <{value}> is not subdomain of the domain of the \
                     identity <{named}>."
                )
            } else {
                format!(
                    "MailFromDomain must be a domain name of the form \
                     [SubDomain.]Domain.TopLevelDomain, got {value:?}"
                )
            };
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {label} ({value:?}) on {named:?}: expected the refusal to say why, got \
                 {message:?}"
            );
        }
        // A refused request writes nothing: the setup already stored stands, and
        // the identity that never had one still has none.
        let actual = reported_for(&router, identity).await;
        assert_eq!(
            actual,
            (
                stored.to_string(),
                PENDING.to_string(),
                "RejectMessage".to_string()
            ),
            "case {label} ({value:?}): a refused request must leave the stored setup alone"
        );
        let actual = reported_for(&router, email_identity).await;
        assert_eq!(
            actual,
            (
                String::new(),
                String::new(),
                USE_DEFAULT_VALUE.to_string()
            ),
            "case {label} ({value:?}): a refused request must store nothing"
        );
    }
}

#[tokio::test]
async fn a_behaviour_outside_the_models_value_set_is_refused() {
    let router = emulator();
    let identity = "behaviour.example.com";
    verify_domain(&router, identity).await;
    let stored = "mail.behaviour.example.com";
    set_mail_from_ok(&router, identity, Some(stored), Some("RejectMessage")).await;

    // Values differing from `BEHAVIOR_ON_M_X_FAILURE_VALUES` in case, in spelling
    // and in being absent from it at all. The model declares the set, so such a
    // value names no behaviour a send could take and the request is refused with
    // HTTP 400 before anything is stored.
    for value in [
        "rejectmessage",
        "REJECTMESSAGE",
        "Reject",
        "UseDefault",
        "usedefaultvalue",
        "Pending",
        "",
    ] {
        let (status, body) =
            set_mail_from(&router, identity, Some("other.example.com"), Some(value)).await;
        assert_eq!(
            status, 400,
            "case BehaviorOnMXFailure={value:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            reported_for(&router, identity).await,
            (
                stored.to_string(),
                PENDING.to_string(),
                "RejectMessage".to_string()
            ),
            "case BehaviorOnMXFailure={value:?}: a refused request must store nothing"
        );
    }
}

#[tokio::test]
async fn a_pending_setup_reaches_success_once_the_clock_owes_it() {
    // SES reports the setup `Pending` until it has read the MAIL FROM domain's MX
    // record; the move is the service's own, so it is owed by the clock rather
    // than by a caller. The span is not zero, so the caller that wrote the setup
    // still sees `Pending`.
    let router = emulator();
    let identity = "pending.example.com";
    verify_domain(&router, identity).await;
    set_mail_from_ok(&router, identity, Some("mail.pending.example.com"), None).await;

    let before = reported_for(&router, identity).await;
    assert_eq!(
        before.1, PENDING,
        "case straight after the call: expected {PENDING}, got {before:?}"
    );
    // Reporting is not what moves it: a second read before the span is up says
    // the same thing.
    let again = reported_for(&router, identity).await;
    assert_eq!(
        again, before,
        "case read twice before the span: expected {before:?}, got {again:?}"
    );

    tokio::time::sleep(MAIL_FROM_SETUP_SPAN + std::time::Duration::from_millis(250)).await;
    let after = reported_for(&router, identity).await;
    assert_eq!(
        after,
        (
            "mail.pending.example.com".to_string(),
            SUCCESS.to_string(),
            USE_DEFAULT_VALUE.to_string()
        ),
        "case past the span: expected {SUCCESS}, got {after:?}"
    );
    // And a settled setup stays settled, so a later reader sees what the one
    // before it saw.
    let settled = reported_for(&router, identity).await;
    assert_eq!(
        settled, after,
        "case read twice past the span: expected {after:?}, got {settled:?}"
    );

    // Naming the SAME domain again changes only the behaviour, so the status the
    // setup has reached stands; naming a DIFFERENT domain is a new setup, which
    // starts `Pending` again.
    set_mail_from_ok(
        &router,
        identity,
        Some("mail.pending.example.com"),
        Some("RejectMessage"),
    )
    .await;
    let same = reported_for(&router, identity).await;
    assert_eq!(
        same,
        (
            "mail.pending.example.com".to_string(),
            SUCCESS.to_string(),
            "RejectMessage".to_string()
        ),
        "case the same domain again: expected the reached status to stand, got {same:?}"
    );
    set_mail_from_ok(&router, identity, Some("bounce.pending.example.com"), None).await;
    let replaced = reported_for(&router, identity).await;
    assert_eq!(
        replaced,
        (
            "bounce.pending.example.com".to_string(),
            PENDING.to_string(),
            USE_DEFAULT_VALUE.to_string()
        ),
        "case a different domain: expected a fresh {PENDING} setup, got {replaced:?}"
    );
}

#[tokio::test]
async fn an_omitted_or_blank_mail_from_domain_disables_the_custom_setting() {
    // `MailFromDomain`'s own documentation: "A value of null disables the custom
    // MAIL FROM setting for the identity." On this wire a null value arrives as
    // an empty field.
    let disabling = [("omitted", None), ("empty", Some("")), ("blank", Some("   "))];
    for (label, value) in disabling {
        let router = emulator();
        let identity = "disable.example.com";
        verify_domain(&router, identity).await;
        let configured = "mail.disable.example.com";
        set_mail_from_ok(&router, identity, Some(configured), Some("RejectMessage")).await;

        set_mail_from_ok(&router, identity, value, None).await;
        let actual = reported_for(&router, identity).await;
        assert_eq!(
            actual,
            (
                String::new(),
                String::new(),
                USE_DEFAULT_VALUE.to_string()
            ),
            "case {label}: expected the custom MAIL FROM setting disabled, got {actual:?}"
        );

        // A disabled identity takes a custom domain again.
        set_mail_from_ok(&router, identity, Some(configured), None).await;
        let actual = reported_for(&router, identity).await;
        assert_eq!(
            actual,
            (
                configured.to_string(),
                PENDING.to_string(),
                USE_DEFAULT_VALUE.to_string()
            ),
            "case {label}: a disabled identity must take a domain again, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_identity_is_named_by_its_name_or_by_its_arn() {
    // The `Identity` member's own documentation: an identity is named "by using
    // its name or by using its Amazon Resource Name (ARN)", and both name the one
    // record — while an ARN of another account, Region or resource names nothing
    // here.
    let router = emulator();
    let identity = "arns.example.com";
    verify_domain(&router, identity).await;
    let domain = "mail.arns.example.com";

    // Written through the ARN, read back under the identity's own name.
    set_mail_from_ok(&router, &identity_arn(identity), Some(domain), None).await;
    let actual = reported_for(&router, identity).await;
    assert_eq!(
        actual,
        (
            domain.to_string(),
            PENDING.to_string(),
            USE_DEFAULT_VALUE.to_string()
        ),
        "case written by ARN: expected the setup on the named identity, got {actual:?}"
    );
    // And read back through the ARN, which resolves to the same record.
    let by_arn = mail_from_attributes(&router, &[&identity_arn(identity)]).await;
    assert_eq!(
        by_arn.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
        vec![identity],
        "case read by ARN: expected the report keyed by the identity, got {by_arn:?}"
    );

    let refused = [
        (
            "another account",
            "arn:aws:ses:us-east-1:999999999999:identity/arns.example.com".to_string(),
        ),
        (
            "another Region",
            engine::emulator_arn("ses", "eu-west-1", &format!("identity/{identity}")),
        ),
        (
            "another service",
            engine::emulator_arn("sns", REGION, &format!("identity/{identity}")),
        ),
        (
            "another resource type",
            engine::emulator_arn("ses", REGION, &format!("configuration-set/{identity}")),
        ),
    ];
    for (label, named) in &refused {
        let (status, body) = set_mail_from(&router, named, Some(domain), None).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {label} ({named:?}): expected 400 InvalidParameterValue, got {status} with \
             {body}"
        );
        let report = mail_from_attributes(&router, &[named]).await;
        assert!(
            report.is_empty(),
            "case {label} ({named:?}): expected no attributes, got {report:?}"
        );
    }
}

#[tokio::test]
async fn an_unverified_identity_is_refused_and_only_verified_ones_are_reported() {
    let router = emulator();
    // One identity IS verified, so the refusals below are about the identity the
    // request names and not about an empty store.
    verify_domain(&router, "verified.example.com").await;
    // And one is verified and then deleted, which must read as unverified again.
    verify_domain(&router, "deleted.example.com").await;
    let (status, body) = call(
        &router,
        "DeleteIdentity",
        &[field("Identity", "deleted.example.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteIdentity(deleted.example.com): expected 200, got {status} with {body}"
    );

    let unverified = [
        ("never verified", "stranger.example.com"),
        ("verified then deleted", "deleted.example.com"),
        ("a subdomain of a verified domain", "sub.verified.example.com"),
    ];
    for (label, named) in &unverified {
        let (status, body) = set_mail_from(&router, named, Some("mail.example.com"), None).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {label} ({named:?}): expected 400 InvalidParameterValue, got {status} with \
             {body}"
        );
        let report = mail_from_attributes(&router, &[named]).await;
        assert!(
            report.is_empty(),
            "case {label} ({named:?}): nothing may be stored for an unverified identity, got \
             {report:?}"
        );
    }

    // "Can only get custom MAIL FROM attributes for up to 100 identities at a
    // time" (the reporting operation's own documentation): the published bound,
    // one below it and one past it.
    for (count, expected_status) in [(1_usize, 200), (99, 200), (100, 200), (101, 400)] {
        let identities: Vec<String> = (0..count)
            .map(|index| format!("identity{index}.example.com"))
            .collect();
        let params: Vec<Field> = identities
            .iter()
            .enumerate()
            .map(|(index, identity)| field(format!("Identities.member.{}", index + 1), identity))
            .collect();
        let (status, body) = call(&router, "GetIdentityMailFromDomainAttributes", &params).await;
        assert_eq!(
            status, expected_status,
            "case {count} identities: expected {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("InvalidParameterValue"),
                "case {count} identities: expected InvalidParameterValue, got {body}"
            );
        }
    }
}
