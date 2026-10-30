// SPDX-License-Identifier: CC0-1.0

//! What a refused custom open-and-click redirect domain is told, end to end over
//! the wire.
//!
//! The rule under test: a tracking-options write refuses a `CustomRedirectDomain`
//! Amazon SES would not redirect through with `InvalidTrackingOptions` (HTTP 400)
//! and a sentence that NAMES the submitted domain — one sentence for a value that
//! is not a domain name (`Domain <value> is not a valid domain.`) and another for
//! a domain name no verified identity of the account covers (`Domain <value> is
//! not verified under this account.`) — while a domain the account's verified
//! identities do cover is stored and reported back.
//!
//! The rows differ along what the rule generalises over: which condition fired,
//! the FORM of the submitted value (empty, padded, a scheme, an empty label, an
//! underscore, a path, an address literal, a single label, a fully qualified
//! name), its length and label count, which operation the write went through
//! (`CreateConfigurationSetTrackingOptions` and
//! `UpdateConfigurationSetTrackingOptions`), and what the named configuration set
//! is (one carrying an association, one carrying none, one the account does not
//! hold) — because the domain is read before the set is looked up, so the
//! message is the same in every one of those contexts.
//!
//! The recorded case that filed the issue is one row of each class.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications, Field,
};
use smithy_server_runtime::MultiServiceRouter;

/// The domain identity every row is judged against: an apex the account has
/// verified, which covers the names under it.
const VERIFIED_APEX: &str = "tracked-apex.example.com";

/// A domain identity BELOW an apex the account has not verified, so coverage is
/// judged from the identity that exists rather than from the apex of the name.
const VERIFIED_SUBDOMAIN: &str = "mail.tracked-sub.example.org";

/// The two conditions `InvalidTrackingOptionsException` lists — "the tracking
/// domain you specified is not a valid domain or subdomain" and "the tracking
/// domain you specified is not verified in Amazon SES" — each in the sentence the
/// runtime reports it with, naming the domain the request supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The submitted value is not a domain name at all.
    NotADomain,
    /// A domain name no verified identity of this account covers.
    NotVerified,
}

impl Refusal {
    /// The message this refusal carries for `submitted`.
    fn message(self, submitted: &str) -> String {
        match self {
            Self::NotADomain => format!("Domain <{submitted}> is not a valid domain."),
            Self::NotVerified => {
                format!("Domain <{submitted}> is not verified under this account.")
            }
        }
    }
}

/// One refused domain: what it is, the value the request carries, and which
/// condition refuses it.
struct Refused {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The `CustomRedirectDomain` the request carries, verbatim.
    submitted: &'static str,
    /// The condition the write is refused under.
    refusal: Refusal,
}

/// The refused rows, spanning both conditions and the forms a submitted value
/// takes.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a scheme and an empty label",
            submitted: "http://bad_domain..example.com",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "an empty value",
            submitted: "",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "a single label",
            submitted: "localhost",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "an underscore in a label",
            submitted: "click_tracking.example.com",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "a padded name",
            submitted: " click.tracked-apex.example.com ",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "two trailing dots",
            submitted: "click.tracked-apex.example.com..",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "a path appended",
            submitted: "click.tracked-apex.example.com/redirect",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "an address literal",
            submitted: "198.51.100.7",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "an all-numeric top-level label",
            submitted: "click.example.123",
            refusal: Refusal::NotADomain,
        },
        Refused {
            // The value goes into an XML error envelope, so a value carrying the
            // metacharacters the envelope is built from has to come back as the
            // text that was submitted rather than breaking the envelope.
            case: "a value carrying XML metacharacters",
            submitted: "click.<a&b>.example.com",
            refusal: Refusal::NotADomain,
        },
        Refused {
            case: "a sibling apex nobody verified",
            submitted: "click.unverified-apex.example.com",
            refusal: Refusal::NotVerified,
        },
        Refused {
            case: "the verified apex as a prefix of another name",
            submitted: "tracked-apex.example.com.evil.test",
            refusal: Refusal::NotVerified,
        },
        Refused {
            case: "the parent of a verified subdomain identity",
            submitted: "tracked-sub.example.org",
            refusal: Refusal::NotVerified,
        },
        Refused {
            case: "the domain of a verified email identity",
            submitted: "mailbox-only.example.net",
            refusal: Refusal::NotVerified,
        },
        Refused {
            case: "an unverified name written fully qualified",
            submitted: "click.unverified-apex.example.com.",
            refusal: Refusal::NotVerified,
        },
        Refused {
            case: "a long unverified name of many labels",
            submitted: "a.b.c.d.e.f.g.unverified-apex.example.com",
            refusal: Refusal::NotVerified,
        },
    ]
}

/// The configuration set a refused write names, which the domain is judged ahead
/// of: a set carrying an association, a set carrying none, and a name the account
/// does not hold.
const SET_WITH_ASSOCIATION: &str = "tracked-set";
const SET_WITHOUT_ASSOCIATION: &str = "untracked-set";
const ABSENT_SET: &str = "absent-set";

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await
}

/// One tracking-options write: `action` is the create or the update, which carry
/// the same input shape.
async fn write_tracking_options(
    router: &MultiServiceRouter,
    action: &str,
    name: &str,
    custom_redirect_domain: &str,
) -> (u16, String) {
    let params: Vec<Field> = vec![
        field("ConfigurationSetName", name),
        field("TrackingOptions.CustomRedirectDomain", custom_redirect_domain),
    ];
    call(router, action, &params).await
}

/// The two operations that write a configuration set's tracking options, which
/// read one domain rule between them.
const WRITERS: [&str; 2] = [
    "CreateConfigurationSetTrackingOptions",
    "UpdateConfigurationSetTrackingOptions",
];

/// The custom redirect domain `DescribeConfigurationSet` reports for `name`, or
/// `None` when the set carries no tracking options.
async fn described_tracking_domain(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeConfigurationSet",
        &[
            field("ConfigurationSetName", name),
            field("ConfigurationSetAttributeNames.member.1", "trackingOptions"),
        ],
    )
    .await;
    assert!(
        status == 200,
        "case describe {name}: expected 200, got {status} with {body}"
    );
    element(&body, "TrackingOptions")
        .as_deref()
        .and_then(|options| element(options, "CustomRedirectDomain"))
}

/// An account holding the verified identities every row is judged against, one
/// configuration set carrying an association and one carrying none.
async fn account() -> MultiServiceRouter {
    let (router, engine) = emulator_and_engine();
    for domain in [VERIFIED_APEX, VERIFIED_SUBDOMAIN] {
        let (status, body) = call(&router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
        assert!(
            status == 200,
            "case verify {domain}: expected 200, got {status} with {body}"
        );
    }
    // An email identity verifies one mailbox, not the domain around it.
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "ops@mailbox-only.example.net")],
    )
    .await;
    assert!(
        status == 200,
        "case verify ops@mailbox-only.example.net: expected 200, got {status} with {body}"
    );
    // Asking for a domain's verification only STARTS it; these rows are about the
    // association, not about the span the record stays pending for.
    settle_verifications(&engine);

    for name in [SET_WITH_ASSOCIATION, SET_WITHOUT_ASSOCIATION] {
        let (status, body) = create_configuration_set(&router, name).await;
        assert!(
            status == 200,
            "case create {name}: expected 200, got {status} with {body}"
        );
    }
    let (status, body) = write_tracking_options(
        &router,
        "CreateConfigurationSetTrackingOptions",
        SET_WITH_ASSOCIATION,
        &format!("click.{VERIFIED_APEX}"),
    )
    .await;
    assert!(
        status == 200,
        "case create the first association: expected 200, got {status} with {body}"
    );
    router
}

#[tokio::test]
async fn a_refused_tracking_domain_is_named_in_the_refusal() {
    let router = account().await;
    let stored = format!("click.{VERIFIED_APEX}");
    for row in refused_rows() {
        let case = row.case;
        let expected = (
            Some("InvalidTrackingOptions".to_string()),
            Some(row.refusal.message(row.submitted)),
        );
        for writer in WRITERS {
            // The domain is judged BEFORE the configuration set is looked up, so
            // every one of these contexts answers with the same refusal.
            for set in [SET_WITH_ASSOCIATION, SET_WITHOUT_ASSOCIATION, ABSENT_SET] {
                let (status, body) =
                    write_tracking_options(&router, writer, set, row.submitted).await;
                assert!(
                    status == 400,
                    "case {case:?} through {writer} on {set}: expected 400, got {status} with \
                     {body}"
                );
                let answered = error_envelope(&body);
                assert!(
                    answered == expected,
                    "case {case:?} through {writer} on {set}: expected {expected:?}, got \
                     {answered:?}"
                );
            }
        }
        // A refused write stored nothing: the association the account holds is
        // untouched, and the set that carried none still carries none.
        let reported = described_tracking_domain(&router, SET_WITH_ASSOCIATION).await;
        assert!(
            reported.as_deref() == Some(stored.as_str()),
            "case {case:?}: expected the stored domain {stored:?} to be unchanged, got \
             {reported:?}"
        );
        let reported = described_tracking_domain(&router, SET_WITHOUT_ASSOCIATION).await;
        assert!(
            reported.is_none(),
            "case {case:?}: a refused write leaves the set with no tracking options, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_the_accounts_identities_cover_is_stored_and_reported() {
    let router = account().await;
    // A verified identity covers the names BENEATH it, so a label the account has
    // never named before needs no identity, record or zone of its own: the apex
    // itself, one fresh label under it, several, a label carrying digits and
    // dashes, the same name written fully qualified (which names the same domain,
    // so the trailing root label is not part of what is stored), and a name under
    // a verified SUBDOMAIN identity.
    let rows = [
        ("the verified apex itself", VERIFIED_APEX.to_string(), VERIFIED_APEX.to_string()),
        (
            "a fresh label under it",
            format!("never-seen.{VERIFIED_APEX}"),
            format!("never-seen.{VERIFIED_APEX}"),
        ),
        (
            "several fresh labels under it",
            format!("deep.nested.never-seen.{VERIFIED_APEX}"),
            format!("deep.nested.never-seen.{VERIFIED_APEX}"),
        ),
        (
            "a label of digits and dashes",
            format!("click-2024.{VERIFIED_APEX}"),
            format!("click-2024.{VERIFIED_APEX}"),
        ),
        (
            "a fully qualified name",
            format!("fqdn.{VERIFIED_APEX}."),
            format!("fqdn.{VERIFIED_APEX}"),
        ),
        (
            "a label under a verified subdomain identity",
            format!("click.{VERIFIED_SUBDOMAIN}"),
            format!("click.{VERIFIED_SUBDOMAIN}"),
        ),
    ];
    for (case, submitted, stored) in rows {
        let (status, body) = write_tracking_options(
            &router,
            "UpdateConfigurationSetTrackingOptions",
            SET_WITH_ASSOCIATION,
            &submitted,
        )
        .await;
        assert!(
            status == 200,
            "case {case:?} ({submitted:?}): expected 200, got {status} with {body}"
        );
        let reported = described_tracking_domain(&router, SET_WITH_ASSOCIATION).await;
        assert!(
            reported.as_deref() == Some(stored.as_str()),
            "case {case:?} ({submitted:?}): expected the stored domain {stored:?}, got \
             {reported:?}"
        );
    }
}
