// SPDX-License-Identifier: CC0-1.0

//! The `NotFoundException` a missing configuration set is refused with, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the harness the sibling [`crate::configuration_sets`]
//! suite shares, so routing, label decoding, the handler and error serialization are all
//! exercised.
//!
//! The rule these cases pin: an operation that resolves a configuration set by name and
//! does not find it is refused with the single sentence this service words once for that
//! resource kind — `Configuration set <NAME> does not exist.`, where the only variable
//! part is the requested name and SES delimits it with angle brackets — so every
//! operation taking a `ConfigurationSetName` reports a missing set with the same status
//! (404), the same code (`NotFoundException`) and the same message, byte for byte.
//!
//! The tables below span what the rule generalises over: which operation resolves the set
//! (each implemented one, over all four HTTP methods the surface uses, with the name in
//! the path and with the name in the body), and the form of the requested name (one
//! character, letters, hyphens, underscores and digits, mixed case, a name that is itself
//! a sentence fragment, and the longest name the documented rule allows). The last case
//! shows the refusal is about the set's absence and not about the request: the identical
//! call succeeds once the set exists.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// The documented length bound of `ConfigurationSetName`, from the member's own
/// documentation: "The name can contain up to 64 alphanumeric characters, including
/// letters, numbers, hyphens (-) and underscores (_) only."
const MAX_CONFIGURATION_SET_NAME_LEN: usize = 64;

/// The identity the association case writes to, which has to exist so that call gets as
/// far as resolving the configuration set it names.
const IDENTITY: &str = "sender@example.com";

/// The sentence SES words for a configuration set the account does not hold: fixed text
/// around the requested name, which is interpolated inside angle brackets.
fn expected_message(name: &str) -> String {
    format!("Configuration set <{name}> does not exist.")
}

/// The valid `ConfigurationSetName` forms the rule has to hold for: one character,
/// letters with a hyphen, letters with underscores and a digit, a mixed-case name, a name
/// that reads as a sentence fragment (the case the brackets keep readable), and the
/// longest name the documented rule permits.
fn missing_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "absent-set".to_string(),
        "absent_set_9".to_string(),
        "Absent-Set_2".to_string(),
        "does-not-exist".to_string(),
        "z".repeat(MAX_CONFIGURATION_SET_NAME_LEN),
    ]
}

/// Every implemented operation that resolves a configuration set by `name`, each answered
/// once, labelled with the operation it is.
///
/// `CreateConfigurationSetEventDestination` and `GetConfigurationSetEventDestinations`
/// are the two the recorded check drives; the others take the same member and share the
/// same wording, so the rule is asserted on all of them.
async fn refusals_for(ses: &Ses, name: &str) -> Vec<(&'static str, Reply)> {
    vec![
        (
            "CreateConfigurationSetEventDestination",
            ses.call(
                "POST",
                &format!("/v2/email/configuration-sets/{name}/event-destinations"),
                Some(json!({
                    "EventDestinationName": "destination",
                    "EventDestination": {"Enabled": true, "MatchingEventTypes": ["SEND"]},
                })),
            )
            .await,
        ),
        (
            "GetConfigurationSetEventDestinations",
            ses.call(
                "GET",
                &format!("/v2/email/configuration-sets/{name}/event-destinations"),
                None,
            )
            .await,
        ),
        (
            "UpdateConfigurationSetEventDestination",
            ses.call(
                "PUT",
                &format!("/v2/email/configuration-sets/{name}/event-destinations/destination"),
                Some(json!({"EventDestination": {"Enabled": false}})),
            )
            .await,
        ),
        (
            "DeleteConfigurationSetEventDestination",
            ses.call(
                "DELETE",
                &format!("/v2/email/configuration-sets/{name}/event-destinations/destination"),
                None,
            )
            .await,
        ),
        (
            "GetConfigurationSet",
            ses.call("GET", &format!("/v2/email/configuration-sets/{name}"), None)
                .await,
        ),
        (
            "DeleteConfigurationSet",
            ses.call(
                "DELETE",
                &format!("/v2/email/configuration-sets/{name}"),
                None,
            )
            .await,
        ),
        (
            // The only one of these that carries the name in the BODY rather than the
            // path, so the wording cannot be coming from the request line.
            "PutEmailIdentityConfigurationSetAttributes",
            ses.call(
                "PUT",
                &format!("/v2/email/identities/{IDENTITY}/configuration-set"),
                Some(json!({"ConfigurationSetName": name})),
            )
            .await,
        ),
    ]
}

#[tokio::test]
async fn a_missing_configuration_set_is_refused_with_the_bracketed_sentence() {
    let ses = Ses::new();
    // One set and one identity exist, so each refusal is about the name asked for and not
    // about an empty account, and the association case gets past its identity lookup.
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": "present-set"}))
            .await
            .status,
        200,
        "creating the configuration set must answer 200"
    );
    assert_eq!(
        ses.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": IDENTITY})),
        )
        .await
        .status,
        200,
        "creating the email identity must answer 200"
    );

    for name in missing_names() {
        let expected = expected_message(&name);
        for (case, reply) in refusals_for(&ses, &name).await {
            assert_eq!(
                reply.status,
                404,
                "case {case:?} on {name:?}: expected 404, got {} ({})",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                "NotFoundException",
                "case {case:?} on {name:?}: expected NotFoundException, got {:?} ({})",
                reply.error_code(),
                reply.message()
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?} on {name:?}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }

    // The set that does exist was not touched by any refused call.
    let reply = ses
        .call(
            "GET",
            "/v2/email/configuration-sets/present-set/event-destinations",
            None,
        )
        .await;
    assert_eq!(
        reply.status,
        200,
        "reading the live set's destinations: expected 200, got {} ({})",
        reply.status,
        reply.message()
    );
    assert_eq!(
        reply.member("EventDestinations"),
        json!([]),
        "the live set must still hold no destination, got {:?}",
        reply.member("EventDestinations")
    );
}

#[tokio::test]
async fn every_operation_words_a_missing_set_identically() {
    let ses = Ses::new();
    assert_eq!(
        ses.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": IDENTITY})),
        )
        .await
        .status,
        200,
        "creating the email identity must answer 200"
    );

    // One wording per resource kind: whichever call first tells a client the set is not
    // there, it says the same thing.
    for name in missing_names() {
        let answers = refusals_for(&ses, &name).await;
        let (first_case, first) = &answers[0];
        let baseline = first.message();
        assert_eq!(
            baseline,
            expected_message(&name),
            "case {first_case:?} on {name:?}: expected {:?}, got {baseline:?}",
            expected_message(&name)
        );
        for (case, reply) in &answers[1..] {
            assert_eq!(
                reply.message(),
                baseline,
                "case {case:?} on {name:?}: must word the refusal as {first_case:?} does \
                 ({baseline:?}), got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn the_identical_create_succeeds_once_the_configuration_set_exists() {
    let ses = Ses::new();
    let definition = json!({"Enabled": true, "MatchingEventTypes": ["SEND", "DELIVERY"]});

    for name in missing_names() {
        let path = format!("/v2/email/configuration-sets/{name}/event-destinations");
        let body = json!({"EventDestinationName": "destination", "EventDestination": definition});

        let refused = ses.call("POST", &path, Some(body.clone())).await;
        assert_eq!(
            refused.message(),
            expected_message(&name),
            "case {name:?}: before the set exists, expected {:?}, got {:?}",
            expected_message(&name),
            refused.message()
        );

        assert_eq!(
            ses.create_set(json!({"ConfigurationSetName": name}))
                .await
                .status,
            200,
            "case {name:?}: creating the configuration set must answer 200"
        );

        let created = ses.call("POST", &path, Some(body)).await;
        assert_eq!(
            created.status,
            200,
            "case {name:?}: the identical create must answer 200 once the set exists, got {} ({})",
            created.status,
            created.message()
        );

        let read = ses.call("GET", &path, None).await;
        let reported = read.member("EventDestinations");
        let names: Vec<&str> = reported
            .as_array()
            .map(|destinations| {
                destinations
                    .iter()
                    .filter_map(|destination| destination.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec!["destination"],
            "case {name:?}: the created destination must be reported back, got {reported:?}"
        );
    }
}
