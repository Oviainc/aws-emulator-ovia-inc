// SPDX-License-Identifier: CC0-1.0

//! Deleting a configuration set, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs:
//!
//! * a delete takes the named set — and the sub-resources the set itself holds —
//!   out of the account's store, so the next reader no longer finds it, for every
//!   form of name the naming rule admits;
//! * a delete naming a set the account does not hold is refused with
//!   `ConfigurationSetDoesNotExist`, whatever the name, which is what a second
//!   delete of one name gets too;
//! * a delete touches only the set it names: every other stored set, and its
//!   tracking options, is left as it was.

use crate::harness::{call, element, emulator, emulator_and_engine, field, settle_verifications, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await
}

/// `DeleteConfigurationSet`.
async fn delete_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DeleteConfigurationSet",
        &[field("ConfigurationSetName", name)],
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

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// Assert that `name` is not a set the account holds, by the refusal its reader
/// answers with.
async fn assert_absent(router: &MultiServiceRouter, name: &str, case: &str) {
    let (status, body) = describe_configuration_set(router, name, &[]).await;
    assert_eq!(
        status, 400,
        "case {case}: describing {name:?} after the delete must be refused, got {status} \
         with {body}"
    );
    assert_eq!(
        refusal(&body),
        (
            Some("ConfigurationSetDoesNotExist".to_string()),
            Some(format!("Configuration set <{name}> does not exist."))
        ),
        "case {case}: expected the not-found refusal for {name:?}, got {body}"
    );
}

/// Names the documented rule admits (`ConfigurationSet$Name`: "letters (a-z,
/// A-Z), numbers (0-9), underscores (_), or dashes (-)", "64 characters or
/// fewer"), so each of these is a set that can be created and then deleted.
fn accepted_names() -> Vec<String> {
    vec![
        "cs".to_string(),
        "Deleted_Set-2".to_string(),
        "9".to_string(),
        "a".repeat(64),
    ]
}

#[tokio::test]
async fn a_delete_removes_the_set_it_names_for_every_accepted_name() {
    let router = emulator();
    for name in accepted_names() {
        let (status, body) = create_configuration_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: expected 200, got {status} with {body}"
        );
        let (status, body) = describe_configuration_set(&router, &name, &[]).await;
        assert_eq!(
            status, 200,
            "case describe {name:?} before the delete: expected 200, got {status} with {body}"
        );

        let (status, body) = delete_configuration_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case delete {name:?}: expected 200, got {status} with {body}"
        );
        assert!(
            body.contains("<DeleteConfigurationSetResponse"),
            "case delete {name:?}: expected this operation's awsQuery envelope, got {body}"
        );

        // The set is gone for every reader...
        assert_absent(&router, &name, &format!("delete {name:?}")).await;
        // ...so the delete is not idempotent: there is nothing left to delete.
        let (status, body) = delete_configuration_set(&router, &name).await;
        assert_eq!(
            status, 400,
            "case delete {name:?} twice: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{name}> does not exist."))
            ),
            "case delete {name:?} twice: expected the not-found refusal, got {body}"
        );

        // And the name is free again: a create after the delete is a create, not
        // a naming conflict.
        let (status, body) = create_configuration_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case re-create {name:?}: expected 200, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_delete_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    // One set exists, so a refused delete is visibly not a delete of whatever
    // the store does hold.
    let (status, body) = create_configuration_set(&router, "kept-set").await;
    assert_eq!(
        status, 200,
        "case create kept-set: expected 200, got {status} with {body}"
    );

    // Names varying along everything the rule admits — length, case, separators
    // — plus names OUTSIDE it ("has space", "dot.name", 65 characters), which no
    // account can hold either: the only error this operation models is the
    // not-found one, so that is the answer for each of them.
    let mut cases = accepted_names();
    cases.extend([
        "has space".to_string(),
        "dot.name".to_string(),
        "name!".to_string(),
        "a".repeat(65),
    ]);
    for name in cases {
        let (status, body) = delete_configuration_set(&router, &name).await;
        assert_eq!(
            status, 400,
            "case delete absent {name:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{name}> does not exist."))
            ),
            "case delete absent {name:?}: expected the not-found refusal, got {body}"
        );
    }

    // The refused deletes removed nothing.
    let (status, body) = describe_configuration_set(&router, "kept-set", &[]).await;
    assert_eq!(
        status, 200,
        "case describe kept-set: expected 200, got {status} with {body}"
    );
    assert_eq!(
        element(&body, "ConfigurationSet")
            .as_deref()
            .and_then(|set| element(set, "Name")),
        Some("kept-set".to_string()),
        "case describe kept-set: expected the stored set, got {body}"
    );
}

#[tokio::test]
async fn a_delete_takes_the_sets_sub_resources_and_nothing_else() {
    let (router, engine) = emulator_and_engine();
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
    // Asking for a domain's verification only starts it, and a tracking domain
    // has to be under one this account HAS verified: the store is settled past
    // the span the clock owes that record, because this suite is about the
    // sub-resources a delete takes with it.
    settle_verifications(&engine);

    // Two sets, each carrying its own tracking-options association.
    let sets = [("tracked-a", "a.example.com"), ("tracked-b", "b.example.com")];
    for (name, domain) in sets {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name}: expected 200, got {status} with {body}"
        );
        let (status, body) = create_tracking_options(&router, name, domain).await;
        assert_eq!(
            status, 200,
            "case create tracking options for {name}: expected 200, got {status} with {body}"
        );
    }

    let (status, body) = delete_configuration_set(&router, "tracked-a").await;
    assert_eq!(
        status, 200,
        "case delete tracked-a: expected 200, got {status} with {body}"
    );
    assert_absent(&router, "tracked-a", "delete tracked-a").await;

    // The other set — and its association — is untouched.
    let reported = described_tracking_domain(&router, "tracked-b").await;
    assert_eq!(
        reported.as_deref(),
        Some("b.example.com"),
        "case delete tracked-a: tracked-b keeps its own tracking options, got {reported:?}"
    );

    // The association went with the set, so the name re-created afterwards
    // carries none: the delete did not leave the sub-resource behind for it.
    let (status, body) = create_configuration_set(&router, "tracked-a").await;
    assert_eq!(
        status, 200,
        "case re-create tracked-a: expected 200, got {status} with {body}"
    );
    let reported = described_tracking_domain(&router, "tracked-a").await;
    assert_eq!(
        reported, None,
        "case re-create tracked-a: a re-created set carries no tracking options, got {reported:?}"
    );
}
