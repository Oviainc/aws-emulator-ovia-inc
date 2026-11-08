// SPDX-License-Identifier: CC0-1.0

//! Creating a configuration set, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs:
//!
//! * a create records the set under the name the request gives it, so every later
//!   reader that names the set finds the same record — for every form of name the
//!   documented naming rule admits, with the attribute groups a freshly created
//!   set carries;
//! * a name the rule does not admit is refused with `InvalidConfigurationSet`,
//!   and nothing is recorded for it;
//! * a name the account already holds is refused with
//!   `ConfigurationSetAlreadyExists`, leaving the stored set — and what hangs off
//!   it — exactly as it was, so a create is never a replacement;
//! * a create that would grow the store past the account's ceiling is refused
//!   with `LimitExceeded`, and the ceiling is the LAST thing decided: a name
//!   outside the rule, and a name already held, keep their own refusals at the
//!   ceiling too.

use crate::harness::{
    call, element, elements, emulator, emulator_over, field, settle_verifications, xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{
    service_state, MAX_CONFIGURATION_SETS, MAX_CONFIGURATION_SET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A router over an engine this suite keeps, so it can seed or read this
/// service's store beside the calls it makes over the wire.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
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

/// `CreateConfigurationSetTrackingOptions`, the sub-resource a stored set holds,
/// so a refused second create is visibly not a replacement of the set.
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

/// The names `ListConfigurationSets` reports, so a created set is asserted
/// through a second reader of the same store and not only through the one that
/// looks it up by name.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListConfigurationSets", &[]).await;
    assert_eq!(
        status, 200,
        "case ListConfigurationSets: expected 200, got {status} with {body}"
    );
    let page = element(&body, "ConfigurationSets").unwrap_or_default();
    elements(&page, "Name")
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// Names the documented rule admits, varying along everything it ranges over:
/// length (one character up to the ceiling), case, digits, and each separator it
/// names. Source: `ConfigurationSet$Name` — "Contain only letters (a-z, A-Z),
/// numbers (0-9), underscores (_), or dashes (-)" and "Contain 64 characters or
/// fewer".
fn accepted_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "7".to_string(),
        "_".to_string(),
        "-".to_string(),
        "Mixed_Case-Set9".to_string(),
        "ALLCAPS".to_string(),
        "0123456789".to_string(),
        // The published ceiling itself, and one character inside it.
        "n".repeat(MAX_CONFIGURATION_SET_NAME - 1),
        "n".repeat(MAX_CONFIGURATION_SET_NAME),
    ]
}

/// Names the rule does not admit: a character outside the set it names (space,
/// dot, punctuation, slash, a non-ASCII letter, a plus), a name built from no
/// characters at all, and one character past the published ceiling.
fn refused_names() -> Vec<String> {
    vec![
        "has space".to_string(),
        "dot.name".to_string(),
        "name!".to_string(),
        "slash/name".to_string(),
        "naïve".to_string(),
        "plus+set".to_string(),
        "n".repeat(MAX_CONFIGURATION_SET_NAME + 1),
    ]
}

#[tokio::test]
async fn a_create_records_the_set_every_later_reader_finds() {
    let router = emulator();
    for name in accepted_names() {
        let (status, body) = create_configuration_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: a name meeting the rule is accepted, got {status} with {body}"
        );
        assert!(
            body.contains("<CreateConfigurationSetResponse"),
            "case create {name:?}: expected this operation's awsQuery envelope, got {body}"
        );

        // The reader that looks the set up by name finds the record the create
        // wrote, under exactly the name the request gave it.
        let (status, body) =
            describe_configuration_set(&router, &name, &["reputationOptions", "trackingOptions"])
                .await;
        assert_eq!(
            status, 200,
            "case describe {name:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "ConfigurationSet")
                .as_deref()
                .and_then(|set| element(set, "Name")),
            Some(name.clone()),
            "case describe {name:?}: expected the stored name, got {body}"
        );
        // A set SES has just created is not paused, and carries no tracking
        // options: the sub-resource is what its own create adds.
        let reputation = element(&body, "ReputationOptions").unwrap_or_default();
        assert_eq!(
            element(&reputation, "SendingEnabled").as_deref(),
            Some("true"),
            "case describe {name:?}: a created set has sending enabled, got {body}"
        );
        assert_eq!(
            element(&body, "TrackingOptions"),
            None,
            "case describe {name:?}: a created set carries no tracking options, got {body}"
        );
    }

    // And the listing reader reports every one of them, so the create wrote to
    // the one store both readers read.
    let listed = listed_names(&router).await;
    let mut expected = accepted_names();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case listing after the creates: expected every created set, got {listed:?}"
    );
}

#[tokio::test]
async fn a_name_outside_the_rule_is_refused_and_records_nothing() {
    let router = emulator();
    for name in refused_names() {
        let (status, body) = create_configuration_set(&router, &name).await;
        assert_eq!(
            status, 400,
            "case create {name:?}: a name outside the rule is refused, got {status} with {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidConfigurationSet"),
            "case create {name:?}: expected the invalid-name refusal, got {body}"
        );
        // "See the error message for details": the details are the rule itself,
        // both halves of it.
        let message = message.unwrap_or_default();
        assert!(
            message.contains("letters")
                && message.contains(&format!("{MAX_CONFIGURATION_SET_NAME} characters")),
            "case create {name:?}: expected a message stating the naming rule, got {message:?}"
        );

        // Nothing was recorded for it: the reader that would find a stored set
        // refuses instead.
        let (status, body) = describe_configuration_set(&router, &name, &[]).await;
        assert_eq!(
            status, 400,
            "case describe {name:?} after the refusal: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{name}> does not exist."))
            ),
            "case describe {name:?} after the refusal: expected the not-found refusal, got {body}"
        );
    }

    let listed = listed_names(&router).await;
    assert!(
        listed.is_empty(),
        "case listing after only refused creates: expected no stored set, got {listed:?}"
    );

    // An empty name is a name built from no characters, so it names nothing; the
    // boundary may refuse it before the handler runs, so only the refusal and
    // the empty store are asserted here.
    let (status, body) = create_configuration_set(&router, "").await;
    assert_eq!(
        status, 400,
        "case create the empty name: expected 400, got {status} with {body}"
    );
    let listed = listed_names(&router).await;
    assert!(
        listed.is_empty(),
        "case create the empty name: expected no stored set, got {listed:?}"
    );
}

#[tokio::test]
async fn a_second_create_of_one_name_is_a_conflict_not_a_replacement() {
    let (router, engine) = emulator_with_store();
    let (status, body) = call(
        &router,
        "VerifyDomainIdentity",
        &[field("Domain", "example.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case verify example.com: expected 200, got {status} with {body}"
    );
    // The request only ASKS for the domain's verification; a tracking domain has
    // to be under one this account has verified, so the store is settled past the
    // span the clock owes that record — this suite is about what a second create
    // does to a stored set.
    settle_verifications(&engine);

    // Each row is a set whose stored record carries something a replacement
    // would lose: a tracking-options association, a paused sending state, or
    // both.
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("plain-set", None, true),
        ("tracked-set", Some("track.example.com"), true),
        ("paused-set", None, false),
        ("paused-tracked-set", Some("clicks.example.com"), false),
    ];
    for (name, tracking_domain, sending_enabled) in rows {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name}: expected 200, got {status} with {body}"
        );
        if let Some(domain) = tracking_domain {
            let (status, body) = create_tracking_options(&router, name, domain).await;
            assert_eq!(
                status, 200,
                "case tracking options for {name}: expected 200, got {status} with {body}"
            );
        }
        if !*sending_enabled {
            // The paused state is written through the operation that owns it, which is
            // where the pause a `SendEmail` is refused under is recorded: the stored set is
            // one the ACCOUNT holds — the SES v2 surface answers for it too
            // (`service_common::ses_configuration_sets`) — so a write straight into this
            // service's own store would be a change the account's record never heard about.
            let (status, body) = call(
                &router,
                "UpdateConfigurationSetSendingEnabled",
                &[
                    field("ConfigurationSetName", *name),
                    field("Enabled", "false"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case pause {name}: expected 200, got {status} with {body}"
            );
        }
    }

    for (name, tracking_domain, sending_enabled) in rows {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case re-create {name}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetAlreadyExists".to_string()),
                Some(format!("Configuration set <{name}> already exists."))
            ),
            "case re-create {name}: expected the already-exists refusal, got {body}"
        );

        // The stored record is the one the first create (and the writes after
        // it) left: the refused create replaced nothing.
        let (status, body) =
            describe_configuration_set(&router, name, &["reputationOptions", "trackingOptions"])
                .await;
        assert_eq!(
            status, 200,
            "case describe {name} after the conflict: expected 200, got {status} with {body}"
        );
        let reported = element(&body, "TrackingOptions")
            .as_deref()
            .and_then(|options| element(options, "CustomRedirectDomain"));
        assert_eq!(
            reported.as_deref(),
            *tracking_domain,
            "case describe {name} after the conflict: expected the stored tracking domain \
             {tracking_domain:?}, got {reported:?}"
        );
        let reputation = element(&body, "ReputationOptions").unwrap_or_default();
        let reported = element(&reputation, "SendingEnabled");
        assert_eq!(
            reported.as_deref(),
            Some(if *sending_enabled { "true" } else { "false" }),
            "case describe {name} after the conflict: expected the stored sending state \
             {sending_enabled}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_create_past_the_accounts_limit_is_refused() {
    let (router, engine) = emulator_with_store();
    {
        // The account's ceiling is 10,000 sets; filling it over the wire would
        // say nothing the store does not already decide, so it is filled
        // through the same state API the handler writes through.
        let store = service_state(&*engine).expect("this service has a store on the shared engine");
        let mut store = store.lock();
        for index in 0..MAX_CONFIGURATION_SETS {
            let name = format!("seeded-{index}");
            assert!(
                store.add_configuration_set(&name).is_some(),
                "case seed {name}: a create inside the limit must be recorded"
            );
        }
    }

    // A NEW name is what the ceiling refuses, whatever form it takes.
    for name in ["one-too-many", "ONE_TOO-MANY2"] {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case create {name} at the ceiling: expected 400, got {status} with {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("LimitExceeded"),
            "case create {name} at the ceiling: expected the service-limit refusal, got {body}"
        );
        let message = message.unwrap_or_default();
        assert!(
            message.contains(&MAX_CONFIGURATION_SETS.to_string()) && message.contains(name),
            "case create {name} at the ceiling: expected a message naming the limit and the \
             set, got {message:?}"
        );
    }

    // The ceiling is the LAST thing decided: a name the rule does not admit, and
    // a name the account already holds, keep their own refusals at it.
    let (status, body) = create_configuration_set(&router, "has space").await;
    assert_eq!(
        status, 400,
        "case create \"has space\" at the ceiling: expected 400, got {status} with {body}"
    );
    assert_eq!(
        refusal(&body).0.as_deref(),
        Some("InvalidConfigurationSet"),
        "case create \"has space\" at the ceiling: the naming rule is decided first, got {body}"
    );
    let (status, body) = create_configuration_set(&router, "seeded-0").await;
    assert_eq!(
        status, 400,
        "case create seeded-0 at the ceiling: expected 400, got {status} with {body}"
    );
    assert_eq!(
        refusal(&body),
        (
            Some("ConfigurationSetAlreadyExists".to_string()),
            Some("Configuration set <seeded-0> already exists.".to_string())
        ),
        "case create seeded-0 at the ceiling: a held name is a conflict, got {body}"
    );

    // And no refused create grew the store past the ceiling.
    let store = service_state(&*engine).expect("this service has a store on the shared engine");
    let held = store.lock().configuration_sets.len();
    assert_eq!(
        held, MAX_CONFIGURATION_SETS,
        "case refused creates at the ceiling: expected {MAX_CONFIGURATION_SETS} sets, got {held}"
    );
}
