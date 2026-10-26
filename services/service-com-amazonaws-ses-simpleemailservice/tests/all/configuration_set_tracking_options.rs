// SPDX-License-Identifier: CC0-1.0

//! The configuration set's custom open-and-click tracking domain, end to end
//! over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE each operation follows over
//! a SET of inputs:
//!
//! * a tracking-options write stores the domain it was given, and the next
//!   `DescribeConfigurationSet` reports exactly that domain — for every form of
//!   domain the rule admits;
//! * a write naming a configuration set the account does not hold is refused
//!   with `ConfigurationSetDoesNotExist`, whatever the name;
//! * a domain Amazon SES would not redirect through — not a domain name, or not
//!   covered by a verified identity — is refused with `InvalidTrackingOptions`,
//!   before the set is looked up at all, and changes nothing;
//! * the association must exist before an update modifies it, and a second
//!   create does not replace it;
//! * a configuration set name is admitted exactly when it meets the documented
//!   naming rule.

use crate::harness::{call, element, emulator, field, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// `VerifyDomainIdentity`, so a tracking domain under it is one SES would
/// redirect through.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "case verify {domain}: expected 200, got {status} with {body}"
    );
}

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await
}

/// `CreateConfigurationSetTrackingOptions`.
async fn create_tracking_options(
    router: &MultiServiceRouter,
    name: &str,
    custom_redirect_domain: &str,
) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSetTrackingOptions",
        &[
            field("ConfigurationSetName", name),
            field("TrackingOptions.CustomRedirectDomain", custom_redirect_domain),
        ],
    )
    .await
}

/// `UpdateConfigurationSetTrackingOptions`.
async fn update_tracking_options(
    router: &MultiServiceRouter,
    name: &str,
    custom_redirect_domain: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateConfigurationSetTrackingOptions",
        &[
            field("ConfigurationSetName", name),
            field("TrackingOptions.CustomRedirectDomain", custom_redirect_domain),
        ],
    )
    .await
}

/// `DescribeConfigurationSet`, asking for the attribute groups in `attributes`.
async fn describe_configuration_set(
    router: &MultiServiceRouter,
    name: &str,
    attributes: &[&str],
) -> (u16, String) {
    let mut params: Vec<Field> = vec![field("ConfigurationSetName", name)];
    for (index, attribute) in attributes.iter().enumerate() {
        params.push(field(
            format!("ConfigurationSetAttributeNames.member.{}", index + 1),
            *attribute,
        ));
    }
    call(router, "DescribeConfigurationSet", &params).await
}

/// The custom redirect domain `DescribeConfigurationSet` reports for `name`, or
/// `None` when the set carries no tracking options.
async fn described_tracking_domain(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, body) = describe_configuration_set(router, name, &["trackingOptions"]).await;
    assert_eq!(
        status, 200,
        "case describe {name}: expected 200, got {status} with {body}"
    );
    element(&body, "TrackingOptions")
        .as_deref()
        .and_then(|tracking_options| element(tracking_options, "CustomRedirectDomain"))
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes
/// a message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// The two sentences `InvalidTrackingOptionsException` documents, which are the
/// messages a refused custom redirect domain carries: one for a value that is
/// not a domain name, one for a domain no verified identity covers.
const NOT_A_DOMAIN: &str = "The tracking domain you specified is not a valid domain or subdomain.";
const NOT_VERIFIED: &str = "The tracking domain you specified is not verified in Amazon SES.";

/// A set with an existing tracking-options association, over an engine that
/// already holds a verified domain identity: the fixture every case about
/// MODIFYING an association starts from.
async fn router_with_tracked_set(name: &str) -> MultiServiceRouter {
    let router = emulator();
    verify_domain(&router, "example.com").await;
    let (status, body) = create_configuration_set(&router, name).await;
    assert_eq!(
        status, 200,
        "case create {name}: expected 200, got {status} with {body}"
    );
    let (status, body) = create_tracking_options(&router, name, "click.example.com").await;
    assert_eq!(
        status, 200,
        "case create tracking options for {name}: expected 200, got {status} with {body}"
    );
    router
}

#[tokio::test]
async fn an_update_stores_the_domain_every_accepted_form_names() {
    let router = router_with_tracked_set("tracked-set").await;
    // A verified domain covers the subdomains under it, so each of these is a
    // domain SES would redirect through: the apex itself, one label under it,
    // several labels under it, a label carrying digits and dashes, and the same
    // name written as a fully qualified (trailing-dot) domain, which names the
    // same domain.
    let cases = [
        ("track.example.com", "track.example.com"),
        ("deep.nested.track.example.com", "deep.nested.track.example.com"),
        ("a-1.example.com", "a-1.example.com"),
        ("example.com", "example.com"),
        ("click.example.com.", "click.example.com"),
    ];
    for (requested, stored) in cases {
        let (status, body) = update_tracking_options(&router, "tracked-set", requested).await;
        assert_eq!(
            status, 200,
            "case update to {requested:?}: expected 200, got {status} with {body}"
        );
        let reported = described_tracking_domain(&router, "tracked-set").await;
        assert_eq!(
            reported.as_deref(),
            Some(stored),
            "case update to {requested:?}: expected the stored domain {stored:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_write_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    verify_domain(&router, "example.com").await;
    // The name varies along everything the rule admits — length, case,
    // separators — and none of these sets was ever created.
    let cases = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for name in cases {
        let expected = format!("Configuration set <{name}> does not exist.");
        for operation in ["update", "create"] {
            let (status, body) = if operation == "update" {
                update_tracking_options(&router, &name, "click.example.com").await
            } else {
                create_tracking_options(&router, &name, "click.example.com").await
            };
            assert_eq!(
                status, 400,
                "case {operation} {name:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                refusal(&body),
                (
                    Some("ConfigurationSetDoesNotExist".to_string()),
                    Some(expected.clone())
                ),
                "case {operation} {name:?}: expected the not-found refusal, got {body}"
            );

            // The tracking options the request SUPPLIES are read before the set
            // is looked up, so a domain the account has not verified is what the
            // same call is refused for — even though the set is not there
            // either.
            let (status, body) = if operation == "update" {
                update_tracking_options(&router, &name, "click.example.org").await
            } else {
                create_tracking_options(&router, &name, "click.example.org").await
            };
            assert_eq!(
                status, 400,
                "case {operation} {name:?} with an unverified domain: expected 400, got {status} \
                 with {body}"
            );
            assert_eq!(
                refusal(&body),
                (
                    Some("InvalidTrackingOptions".to_string()),
                    Some(NOT_VERIFIED.to_string())
                ),
                "case {operation} {name:?} with an unverified domain: expected the \
                 invalid-tracking-options refusal, got {body}"
            );
        }

        // The refused writes stored nothing: the set is still not there.
        let (status, body) = describe_configuration_set(&router, &name, &["trackingOptions"]).await;
        assert_eq!(
            status, 400,
            "case describe {name:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(expected)
            ),
            "case describe {name:?}: expected the not-found refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn a_domain_ses_would_not_redirect_through_is_refused() {
    let router = router_with_tracked_set("tracked-set").await;
    // An email identity verifies one mailbox, not the domain around it, so
    // `other.example` is as unverified as a domain nobody named.
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "ops@other.example")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case verify ops@other.example: expected 200, got {status} with {body}"
    );

    // Two refusals, both `InvalidTrackingOptions`: a value that is not a domain
    // name at all — which a padded or doubly dotted name is, since only the one
    // root label of a fully qualified name is taken off — and a domain name no
    // verified identity covers.
    let cases = [
        ("", NOT_A_DOMAIN),
        ("localhost", NOT_A_DOMAIN),
        ("not a domain", NOT_A_DOMAIN),
        (" click.example.com ", NOT_A_DOMAIN),
        ("click.example.com..", NOT_A_DOMAIN),
        ("click.example.com/path", NOT_A_DOMAIN),
        ("10.0.0.1", NOT_A_DOMAIN),
        ("click.example.org", NOT_VERIFIED),
        ("other.example", NOT_VERIFIED),
        ("example.com.evil.test", NOT_VERIFIED),
    ];
    for (domain, expected) in cases {
        for operation in ["update", "create"] {
            let (status, body) = if operation == "update" {
                update_tracking_options(&router, "tracked-set", domain).await
            } else {
                create_tracking_options(&router, "tracked-set", domain).await
            };
            assert_eq!(
                status, 400,
                "case {operation} to {domain:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                refusal(&body),
                (
                    Some("InvalidTrackingOptions".to_string()),
                    Some(expected.to_string())
                ),
                "case {operation} to {domain:?}: expected the invalid-tracking-options refusal, \
                 got {body}"
            );
        }
        // A refused write leaves the stored association alone.
        let reported = described_tracking_domain(&router, "tracked-set").await;
        assert_eq!(
            reported.as_deref(),
            Some("click.example.com"),
            "case {domain:?}: the stored domain must be unchanged, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_association_must_exist_before_an_update_modifies_it() {
    let router = emulator();
    verify_domain(&router, "example.com").await;
    let (status, body) = create_configuration_set(&router, "fresh-set").await;
    assert_eq!(
        status, 200,
        "case create fresh-set: expected 200, got {status} with {body}"
    );
    // A set created without tracking options carries none...
    let reported = described_tracking_domain(&router, "fresh-set").await;
    assert_eq!(
        reported, None,
        "case fresh-set: a new set carries no tracking options, got {reported:?}"
    );
    // ...so there is nothing for an update to modify, for ANY domain it would
    // otherwise accept.
    for domain in ["click.example.com", "track.example.com"] {
        let (status, body) = update_tracking_options(&router, "fresh-set", domain).await;
        assert_eq!(
            status, 400,
            "case update fresh-set to {domain:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("TrackingOptionsDoesNotExistException".to_string()),
                Some("The TrackingOptions object you specified does not exist.".to_string())
            ),
            "case update fresh-set to {domain:?}: expected the no-such-association refusal, \
             got {body}"
        );
        let reported = described_tracking_domain(&router, "fresh-set").await;
        assert_eq!(
            reported, None,
            "case update fresh-set to {domain:?}: a refused update stores nothing, got {reported:?}"
        );
    }

    // The create is what adds the association; a second one does not replace it,
    // and the update then does.
    let (status, body) = create_tracking_options(&router, "fresh-set", "click.example.com").await;
    assert_eq!(
        status, 200,
        "case create tracking options: expected 200, got {status} with {body}"
    );
    let (status, body) = create_tracking_options(&router, "fresh-set", "track.example.com").await;
    assert_eq!(
        status, 400,
        "case create tracking options twice: expected 400, got {status} with {body}"
    );
    assert_eq!(
        refusal(&body),
        (
            Some("TrackingOptionsAlreadyExistsException".to_string()),
            Some(
                "The configuration set you specified already contains a TrackingOptions object."
                    .to_string()
            )
        ),
        "case create tracking options twice: expected the already-exists refusal, got {body}"
    );
    let reported = described_tracking_domain(&router, "fresh-set").await;
    assert_eq!(
        reported.as_deref(),
        Some("click.example.com"),
        "case create tracking options twice: the first association stands, got {reported:?}"
    );
    let (status, body) = update_tracking_options(&router, "fresh-set", "track.example.com").await;
    assert_eq!(
        status, 200,
        "case update after create: expected 200, got {status} with {body}"
    );
    let reported = described_tracking_domain(&router, "fresh-set").await;
    assert_eq!(
        reported.as_deref(),
        Some("track.example.com"),
        "case update after create: expected the updated domain, got {reported:?}"
    );
}

#[tokio::test]
async fn a_configuration_set_name_is_admitted_exactly_when_the_rule_admits_it() {
    let router = emulator();
    // `ConfigurationSet$Name`: "Contain only letters (a-z, A-Z), numbers (0-9),
    // underscores (_), or dashes (-)" and "Contain 64 characters or fewer" — so
    // 64 characters is accepted and 65 is not.
    let accepted = ["cs", "Cs_1-2", "9", &"a".repeat(64)];
    for name in accepted {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: a name meeting the rule is accepted, got {status} with {body}"
        );
        let (status, body) = describe_configuration_set(&router, name, &[]).await;
        assert_eq!(
            status, 200,
            "case describe {name:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "ConfigurationSet")
                .as_deref()
                .and_then(|set| element(set, "Name")),
            Some(name.to_string()),
            "case describe {name:?}: expected the stored name, got {body}"
        );
    }

    let refused = ["has space", "dot.name", "name!", &"a".repeat(65)];
    for name in refused {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case create {name:?}: a name outside the rule is refused, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidConfigurationSet"),
            "case create {name:?}: expected the invalid-name refusal, got {body}"
        );
    }

    // The name is a key: the second create of one name is a conflict, not a
    // replacement.
    let (status, body) = create_configuration_set(&router, "cs").await;
    assert_eq!(
        status, 400,
        "case create cs twice: expected 400, got {status} with {body}"
    );
    assert_eq!(
        refusal(&body),
        (
            Some("ConfigurationSetAlreadyExists".to_string()),
            Some("Configuration set <cs> already exists.".to_string())
        ),
        "case create cs twice: expected the already-exists refusal, got {body}"
    );
}
