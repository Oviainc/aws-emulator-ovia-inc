// SPDX-License-Identifier: CC0-1.0

//! A configuration set's own sending (pause) state, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs:
//!
//! * `UpdateConfigurationSetSendingEnabled` writes the flag on the NAMED set, so
//!   the flag `DescribeConfigurationSet` reports as
//!   `ReputationOptions$SendingEnabled` is whatever the last write for that set
//!   left — for every sequence of writes — and no other set, nor the account's
//!   own flag, moves with it;
//! * the stored flag is what a send is checked against: a send naming a set
//!   whose sending is disabled is refused with `ConfigurationSetSendingPaused`,
//!   and a send naming no set, or naming an enabled set, is not;
//! * re-enabling a set whose sending was disabled resets the set's reputation
//!   metrics, which is the `LastFreshStart` the same reader reports — absent
//!   until the first such transition;
//! * a write naming a configuration set the account does not hold is refused
//!   with `ConfigurationSetDoesNotExist`, whatever the name and whichever flag
//!   it asked for, and stores nothing.

use crate::harness::{
    call, element, emulator, emulator_and_engine, field, settle_verifications, xml_text, Field,
};
use smithy_server_runtime::MultiServiceRouter;
use std::time::SystemTime;

/// One call that must succeed, as its XML body.
async fn ok(router: &MultiServiceRouter, action: &str, params: &[Field]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {action}: expected 200, got {status} with {body}"
    );
    body
}

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await;
}

/// `UpdateConfigurationSetSendingEnabled`.
async fn update_sending_enabled(
    router: &MultiServiceRouter,
    name: &str,
    enabled: bool,
) -> (u16, String) {
    call(
        router,
        "UpdateConfigurationSetSendingEnabled",
        &[
            field("ConfigurationSetName", name),
            field("Enabled", enabled.to_string()),
        ],
    )
    .await
}

/// The `ReputationOptions` members an assertion here is about: the set's sending
/// flag, and the fresh start it reports (absent as `None`).
#[derive(Debug, PartialEq, Eq)]
struct Reputation {
    sending_enabled: bool,
    last_fresh_start: Option<String>,
}

/// `DescribeConfigurationSet` asking for `reputationOptions`, as the group it
/// reports.
async fn reported_reputation(router: &MultiServiceRouter, name: &str) -> Reputation {
    let body = ok(
        router,
        "DescribeConfigurationSet",
        &[
            field("ConfigurationSetName", name),
            field("ConfigurationSetAttributeNames.member.1", "reputationOptions"),
        ],
    )
    .await;
    let group = element(&body, "ReputationOptions").unwrap_or_else(|| {
        panic!("case describe {name}: expected a ReputationOptions group, got {body}")
    });
    let sending_enabled = element(&group, "SendingEnabled").unwrap_or_else(|| {
        panic!("case describe {name}: expected a SendingEnabled member, got {group}")
    });
    Reputation {
        sending_enabled: sending_enabled.parse::<bool>().unwrap_or_else(|_| {
            panic!("case describe {name}: SendingEnabled must be a boolean, got {sending_enabled:?}")
        }),
        last_fresh_start: element(&group, "LastFreshStart"),
    }
}

/// `GetAccountSendingEnabled`, as the account-wide flag it reports — the one a
/// per-set write must not touch.
async fn account_sending_enabled(router: &MultiServiceRouter) -> bool {
    let body = ok(router, "GetAccountSendingEnabled", &[]).await;
    let enabled = element(&body, "Enabled")
        .unwrap_or_else(|| panic!("GetAccountSendingEnabled must report Enabled, got {body}"));
    enabled
        .parse::<bool>()
        .unwrap_or_else(|_| panic!("Enabled must be a boolean, got {enabled:?}"))
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// A `SendEmail` request from the verified sender, optionally naming a
/// configuration set.
fn send_params(configuration_set: Option<&str>) -> Vec<Field> {
    let mut params = vec![
        field("Source", "sender@example.com"),
        field("Destination.ToAddresses.member.1", "to@example.net"),
        field("Message.Subject.Data", "Pause state"),
        field("Message.Body.Text.Data", "Body"),
    ];
    if let Some(name) = configuration_set {
        params.push(field("ConfigurationSetName", name));
    }
    params
}

/// A timestamp a response body carries, as seconds since the epoch.
fn wire_seconds(rendered: &str) -> f64 {
    ::aws_smithy_types::DateTime::from_str(rendered, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("a reported timestamp must be a wire timestamp, got {rendered:?}: {error}")
        })
        .as_secs_f64()
}

/// A `SystemTime` the test took, as seconds since the epoch, so a reported
/// timestamp can be bracketed by the call that produced it.
fn clock_seconds(at: SystemTime) -> f64 {
    ::aws_smithy_types::DateTime::from(at).as_secs_f64()
}

#[tokio::test]
async fn the_sets_sending_flag_is_whatever_the_last_write_left() {
    let router = emulator();
    // Two sets, so every row can check that the write reached the NAMED set only.
    create_configuration_set(&router, "paused-set").await;
    create_configuration_set(&router, "other-set").await;

    // A set SES has just created is not paused — the state, not a canned answer:
    // the writes below change what this same reader reports.
    for name in ["paused-set", "other-set"] {
        let reported = reported_reputation(&router, name).await;
        assert!(
            reported.sending_enabled,
            "case fresh set {name}: expected sending enabled, got {reported:?}"
        );
    }

    // Each row is a sequence of writes against `paused-set`; the flag after it
    // must be the LAST value written. The rows span a single write, a change, a
    // repeat of the value already stored (idempotent), and longer alternations.
    let rows: &[(&str, &[bool])] = &[
        ("disable", &[false]),
        ("disable then enable", &[false, true]),
        ("enable twice", &[true, true]),
        ("disable twice", &[false, false]),
        ("alternating", &[true, false, true, false, true]),
        ("ending disabled", &[true, false, true, false]),
    ];
    for (case, writes) in rows {
        for enabled in *writes {
            let (status, body) = update_sending_enabled(&router, "paused-set", *enabled).await;
            assert_eq!(
                status, 200,
                "case {case} writing {enabled}: expected 200, got {status} with {body}"
            );
        }
        let expected = *writes.last().expect("every row writes at least once");
        let reported = reported_reputation(&router, "paused-set").await;
        assert_eq!(
            reported.sending_enabled, expected,
            "case {case}: expected the flag to be {expected} after {writes:?}, got {reported:?}"
        );
        // The write is per set: the other set and the account-wide flag are
        // untouched by any of it.
        let untouched = reported_reputation(&router, "other-set").await;
        assert_eq!(
            untouched,
            Reputation {
                sending_enabled: true,
                last_fresh_start: None
            },
            "case {case}: the set that was not named must be untouched, got {untouched:?}"
        );
        assert!(
            account_sending_enabled(&router).await,
            "case {case}: the account-wide flag must be untouched"
        );
    }

    // The group is reported only when the request asks for it, so a describe
    // that names no attribute carries no sending flag at all.
    let body = ok(
        &router,
        "DescribeConfigurationSet",
        &[field("ConfigurationSetName", "paused-set")],
    )
    .await;
    assert!(
        element(&body, "ReputationOptions").is_none(),
        "case describe without attributes: expected no ReputationOptions group, got {body}"
    );
}

#[tokio::test]
async fn a_paused_set_refuses_a_send_naming_it() {
    let (router, engine) = emulator_and_engine();
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "sender@example.com")],
    )
    .await;
    // The request only ASKS for the address's verification; this suite is about a
    // set's sending flag, so the store is settled past the span the clock owes
    // the record rather than slept through.
    settle_verifications(&engine);
    // Names spanning the forms the documented naming rule admits, so the rule is
    // asserted over the name as well as the flag.
    let names = ["paused-set", "Paused_Set_2", "z", &"a".repeat(64)];
    for name in names {
        create_configuration_set(&router, name).await;
    }

    for name in names {
        for enabled in [false, true, false] {
            let (status, body) = update_sending_enabled(&router, name, enabled).await;
            assert_eq!(
                status, 200,
                "case {name} sending {enabled}: expected 200, got {status} with {body}"
            );

            // The flag is not just reported back: it is the state a send naming
            // the set is checked against, which is what this operation documents
            // ("temporarily pause email sending for a configuration set").
            let (status, body) = call(&router, "SendEmail", &send_params(Some(name))).await;
            let expected_status = if enabled { 200 } else { 400 };
            assert_eq!(
                status, expected_status,
                "case send naming {name} with sending {enabled}: expected {expected_status}, got \
                 {status} with {body}"
            );
            if !enabled {
                assert_eq!(
                    refusal(&body),
                    (
                        Some("ConfigurationSetSendingPausedException".to_string()),
                        Some(format!(
                            "Email sending is disabled for configuration set <{name}>."
                        ))
                    ),
                    "case send naming {name} with sending disabled: expected the paused refusal, \
                     got {body}"
                );
            }

            // A send naming NO configuration set is not governed by any set's
            // flag, so it is accepted whatever the sets hold.
            let (status, body) = call(&router, "SendEmail", &send_params(None)).await;
            assert_eq!(
                status, 200,
                "case send naming no set while {name} has sending {enabled}: expected 200, got \
                 {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn re_enabling_a_disabled_set_resets_its_reputation_metrics() {
    let router = emulator();
    create_configuration_set(&router, "fresh-start-set").await;

    // `ReputationOptions$LastFreshStart`: "If email sending for the configuration
    // set has never been disabled and later re-enabled, the value of this
    // attribute is null." A fresh set, a redundant enable and a disable each
    // leave it that way...
    for (case, writes) in [
        ("fresh set", &[][..]),
        ("enable an already enabled set", &[true]),
        ("enable it again", &[true, true]),
        ("disable it", &[false]),
        ("disable it again", &[false, false]),
    ] {
        for enabled in writes {
            let (status, body) = update_sending_enabled(&router, "fresh-start-set", *enabled).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {body}"
            );
        }
        let reported = reported_reputation(&router, "fresh-start-set").await;
        assert_eq!(
            reported.last_fresh_start, None,
            "case {case}: expected no fresh start yet, got {reported:?}"
        );
    }

    // ...and the disabled-to-enabled transition is what stamps one, at the
    // instant of that call. Twice, because a LATER re-enable resets the metrics
    // again: the stamp moves forward rather than staying at the first one.
    let mut previous: Option<f64> = None;
    for round in 1..=2 {
        let (status, body) = update_sending_enabled(&router, "fresh-start-set", false).await;
        assert_eq!(
            status, 200,
            "case round {round} disable: expected 200, got {status} with {body}"
        );
        let before = clock_seconds(SystemTime::now());
        let (status, body) = update_sending_enabled(&router, "fresh-start-set", true).await;
        assert_eq!(
            status, 200,
            "case round {round} re-enable: expected 200, got {status} with {body}"
        );
        let after = clock_seconds(SystemTime::now());

        let reported = reported_reputation(&router, "fresh-start-set").await;
        let rendered = reported.last_fresh_start.clone().unwrap_or_else(|| {
            panic!("case round {round}: expected a fresh start after a re-enable, got {reported:?}")
        });
        let stamped = wire_seconds(&rendered);
        assert!(
            (before..=after).contains(&stamped),
            "case round {round}: expected the fresh start inside the re-enabling call \
             ({before}..={after}), got {stamped} from {rendered:?}"
        );
        if let Some(previous) = previous {
            assert!(
                stamped >= previous,
                "case round {round}: expected a later fresh start than {previous}, got {stamped}"
            );
        }
        previous = Some(stamped);

        // A read does not move it: the same reader reports the same instant.
        let again = reported_reputation(&router, "fresh-start-set").await;
        assert_eq!(
            again.last_fresh_start.as_deref(),
            Some(rendered.as_str()),
            "case round {round}: the fresh start must be stable across reads, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_write_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    // The name varies along everything the documented naming rule admits —
    // length, case, separators — and none of these sets was ever created.
    let names = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for name in names {
        let expected = format!("Configuration set <{name}> does not exist.");
        for enabled in [false, true] {
            let (status, body) = update_sending_enabled(&router, &name, enabled).await;
            assert_eq!(
                status, 400,
                "case {name:?} sending {enabled}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                refusal(&body),
                (
                    Some("ConfigurationSetDoesNotExist".to_string()),
                    Some(expected.clone())
                ),
                "case {name:?} sending {enabled}: expected the not-found refusal, got {body}"
            );
        }

        // The refused writes stored nothing: the set is still not there.
        let (status, body) = call(
            &router,
            "DescribeConfigurationSet",
            &[field("ConfigurationSetName", &name)],
        )
        .await;
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
