// SPDX-License-Identifier: CC0-1.0

//! The configuration set's delivery options — the TLS policy a send that uses
//! the set is delivered under — end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE each operation follows over a
//! SET of inputs:
//!
//! * a delivery-options write stores exactly the policy it was given, for EVERY
//!   policy `TlsPolicy` names, and the next `DescribeConfigurationSet` reports
//!   that policy back;
//! * the write is a PUT: a later one replaces the stored policy, and one carrying
//!   no delivery options leaves the set carrying none;
//! * the policy is per configuration set — one set's policy says nothing about
//!   another's;
//! * `DescribeConfigurationSet` reports the group only when the request asks for
//!   it;
//! * a write naming a configuration set the account does not hold is refused with
//!   `ConfigurationSetDoesNotExist`, whatever the name, and stores nothing;
//! * a value outside the policies `TlsPolicy` names is refused and changes
//!   nothing.

use crate::harness::{call, element, emulator, field, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// The policies a TLS policy may be, from the model's own `TlsPolicy` enum
/// (`smithy_query(query="shape",shape="TlsPolicy")`: `Require`, `Optional`).
const TLS_POLICIES: &[&str] = &["Require", "Optional"];

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await
}

/// A router holding one configuration set, which is where every case about its
/// delivery options starts.
async fn router_with_set(name: &str) -> MultiServiceRouter {
    let router = emulator();
    let (status, body) = create_configuration_set(&router, name).await;
    assert_eq!(
        status, 200,
        "case create {name}: expected 200, got {status} with {body}"
    );
    router
}

/// `PutConfigurationSetDeliveryOptions` carrying a TLS policy.
async fn put_delivery_options(
    router: &MultiServiceRouter,
    name: &str,
    tls_policy: &str,
) -> (u16, String) {
    call(
        router,
        "PutConfigurationSetDeliveryOptions",
        &[
            field("ConfigurationSetName", name),
            field("DeliveryOptions.TlsPolicy", tls_policy),
        ],
    )
    .await
}

/// `PutConfigurationSetDeliveryOptions` carrying no delivery options at all —
/// which the model admits, since `DeliveryOptions` is not a required member.
async fn put_no_delivery_options(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "PutConfigurationSetDeliveryOptions",
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

/// The TLS policy `DescribeConfigurationSet` reports for `name`, or `None` when
/// the set carries no delivery options.
async fn described_tls_policy(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, body) = describe_configuration_set(router, name, &["deliveryOptions"]).await;
    assert_eq!(
        status, 200,
        "case describe {name}: expected 200, got {status} with {body}"
    );
    element(&body, "DeliveryOptions")
        .as_deref()
        .and_then(|delivery_options| element(delivery_options, "TlsPolicy"))
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

#[tokio::test]
async fn a_write_stores_every_policy_the_model_names_and_describe_reports_it() {
    let router = router_with_set("delivery-set").await;
    // A set created without delivery options carries none: the model documents no
    // default for `DeliveryOptions$TlsPolicy`.
    let reported = described_tls_policy(&router, "delivery-set").await;
    assert_eq!(
        reported, None,
        "case fresh set: a new set carries no delivery options, got {reported:?}"
    );

    for policy in TLS_POLICIES {
        let (status, body) = put_delivery_options(&router, "delivery-set", policy).await;
        assert_eq!(
            status, 200,
            "case put {policy:?}: expected 200, got {status} with {body}"
        );
        let reported = described_tls_policy(&router, "delivery-set").await;
        assert_eq!(
            reported.as_deref(),
            Some(*policy),
            "case put {policy:?}: expected the stored policy {policy:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_second_write_replaces_the_stored_policy_and_one_carrying_none_clears_it() {
    let router = router_with_set("replaced-set").await;
    // The write is a PUT: whatever the request carries becomes the set's delivery
    // options, in any order the policies can be written, and a request carrying
    // no delivery options leaves the set carrying none.
    let steps = [
        (Some("Require"), Some("Require")),
        (Some("Optional"), Some("Optional")),
        (Some("Optional"), Some("Optional")),
        (Some("Require"), Some("Require")),
        (None, None),
        (None, None),
        (Some("Optional"), Some("Optional")),
    ];
    for (index, (requested, stored)) in steps.into_iter().enumerate() {
        let (status, body) = match requested {
            Some(policy) => put_delivery_options(&router, "replaced-set", policy).await,
            None => put_no_delivery_options(&router, "replaced-set").await,
        };
        assert_eq!(
            status, 200,
            "case step {index} put {requested:?}: expected 200, got {status} with {body}"
        );
        let reported = described_tls_policy(&router, "replaced-set").await;
        assert_eq!(
            reported.as_deref(),
            stored,
            "case step {index} put {requested:?}: expected the stored policy {stored:?}, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_policy_is_per_configuration_set() {
    let router = emulator();
    // Each set gets its own policy, and the sets are written in an order that is
    // not the order they are read back in.
    let sets = [
        ("alpha-set", Some("Require")),
        ("beta_set", Some("Optional")),
        ("Gamma-Set-3", None),
    ];
    for (name, policy) in sets {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name}: expected 200, got {status} with {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = put_delivery_options(&router, name, policy).await;
            assert_eq!(
                status, 200,
                "case put {policy:?} on {name}: expected 200, got {status} with {body}"
            );
        }
    }
    for (name, policy) in sets {
        let reported = described_tls_policy(&router, name).await;
        assert_eq!(
            reported.as_deref(),
            policy,
            "case {name}: expected the policy this set was given ({policy:?}), got {reported:?}"
        );
    }
}

#[tokio::test]
async fn describe_reports_the_group_only_when_the_request_asks_for_it() {
    let router = router_with_set("asked-set").await;
    let (status, body) = put_delivery_options(&router, "asked-set", "Require").await;
    assert_eq!(
        status, 200,
        "case put Require: expected 200, got {status} with {body}"
    );
    // The attribute groups a describe reports are the ones the request names, so
    // a request that asks for other groups — or for none — carries no
    // `DeliveryOptions`, however the policy is stored.
    let cases: [(&[&str], bool); 5] = [
        (&[], false),
        (&["reputationOptions"], false),
        (&["trackingOptions", "eventDestinations"], false),
        (&["deliveryOptions"], true),
        (&["reputationOptions", "deliveryOptions"], true),
    ];
    for (attributes, reported) in cases {
        let (status, body) = describe_configuration_set(&router, "asked-set", attributes).await;
        assert_eq!(
            status, 200,
            "case describe {attributes:?}: expected 200, got {status} with {body}"
        );
        let policy = element(&body, "DeliveryOptions")
            .as_deref()
            .and_then(|delivery_options| element(delivery_options, "TlsPolicy"));
        assert_eq!(
            policy.as_deref(),
            reported.then_some("Require"),
            "case describe {attributes:?}: expected the group reported={reported}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_write_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    // The name varies along everything the naming rule admits — length, case,
    // separators — and none of these sets was ever created.
    let names = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for name in names {
        let expected = format!("Configuration set <{name}> does not exist.");
        for policy in TLS_POLICIES {
            let (status, body) = put_delivery_options(&router, &name, policy).await;
            assert_eq!(
                status, 400,
                "case put {policy:?} on {name:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                refusal(&body),
                (
                    Some("ConfigurationSetDoesNotExist".to_string()),
                    Some(expected.clone())
                ),
                "case put {policy:?} on {name:?}: expected the not-found refusal, got {body}"
            );
        }
        // The refused writes stored nothing: the set is still not there.
        let (status, body) = describe_configuration_set(&router, &name, &["deliveryOptions"]).await;
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
async fn a_policy_outside_the_model_s_value_set_is_refused_and_changes_nothing() {
    let router = router_with_set("guarded-set").await;
    let (status, body) = put_delivery_options(&router, "guarded-set", "Require").await;
    assert_eq!(
        status, 200,
        "case put Require: expected 200, got {status} with {body}"
    );

    // `TlsPolicy` names exactly two policies, so each of these is a value no send
    // could ever be delivered under: a name of neither policy, either policy in
    // the wrong case, a policy name with text around it, and the two run
    // together.
    let cases = [
        "NotAPolicy",
        "require",
        "OPTIONAL",
        "Require ",
        " Optional",
        "RequireOptional",
        "Required",
    ];
    for value in cases {
        let (status, body) = put_delivery_options(&router, "guarded-set", value).await;
        assert_eq!(
            status, 400,
            "case put {value:?}: expected 400, got {status} with {body}"
        );
        let (code, message) = refusal(&body);
        let message = message.unwrap_or_default();
        assert!(
            code.is_some() && !message.is_empty(),
            "case put {value:?}: expected a refusal naming a code and a message, got {body}"
        );
        // The refusal says what was wrong with the request: the member it rejected
        // the value at and the set the value had to come from, so the caller can fix
        // the request from the text alone. The rejected value itself is NOT echoed —
        // an enum clause states the admitted set instead (`crate::service::validation`
        // of this crate states that rule for every enum-constrained member).
        assert!(
            message.contains("tlsPolicy")
                && message.contains("Member must satisfy enum value set: [Require, Optional]"),
            "case put {value:?}: expected a message naming the member and the admitted \
             set, got {message:?}"
        );
        // And it stored nothing: the policy the set already carried stands.
        let reported = described_tls_policy(&router, "guarded-set").await;
        assert_eq!(
            reported.as_deref(),
            Some("Require"),
            "case put {value:?}: the stored policy must be unchanged, got {reported:?}"
        );
    }
}
