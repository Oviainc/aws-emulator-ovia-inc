// SPDX-License-Identifier: CC0-1.0

//! Whether Amazon SES publishes a configuration set's reputation metrics, end to
//! end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs:
//!
//! * the flag a write carries is the one stored on the set it names, so the next
//!   `DescribeConfigurationSet` reports exactly that value — for every value and
//!   every order of writes;
//! * the flag belongs to ONE set: writing it on a set leaves every other set's
//!   flag where it was;
//! * a set that was never written to reports the member's documented default;
//! * a write naming a configuration set the account does not hold is refused
//!   with `ConfigurationSetDoesNotExist`, whatever the name, and stores nothing.

use crate::harness::{call, element, emulator, field, xml_text, Field};
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

/// `UpdateConfigurationSetReputationMetricsEnabled`.
async fn update_reputation_metrics(
    router: &MultiServiceRouter,
    name: &str,
    enabled: bool,
) -> (u16, String) {
    call(
        router,
        "UpdateConfigurationSetReputationMetricsEnabled",
        &[
            field("ConfigurationSetName", name),
            field("Enabled", if enabled { "true" } else { "false" }),
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

/// The reputation settings `DescribeConfigurationSet` reports for `name`, as
/// `(ReputationMetricsEnabled, SendingEnabled)`. A member the body leaves out is
/// `None`.
async fn described_reputation(
    router: &MultiServiceRouter,
    name: &str,
) -> (Option<String>, Option<String>) {
    let (status, body) = describe_configuration_set(router, name, &["reputationOptions"]).await;
    assert_eq!(
        status, 200,
        "case describe {name:?}: expected 200, got {status} with {body}"
    );
    let reputation = element(&body, "ReputationOptions").unwrap_or_else(|| {
        panic!("case describe {name:?}: expected a ReputationOptions group, got {body}")
    });
    (
        element(&reputation, "ReputationMetricsEnabled"),
        element(&reputation, "SendingEnabled"),
    )
}

/// The flag `DescribeConfigurationSet` reports for `name`, as the boolean it
/// stands for. The member carries a documented default, so a body that leaves it
/// out is reporting that default.
async fn reported_flag(router: &MultiServiceRouter, name: &str) -> bool {
    let (reported, _) = described_reputation(router, name).await;
    match reported.as_deref() {
        Some("true") => true,
        Some("false") | None => false,
        other => panic!(
            "case describe {name:?}: ReputationMetricsEnabled must be a boolean, got {other:?}"
        ),
    }
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// Configuration-set names spanning everything the documented naming rule
/// admits — `ConfigurationSet$Name`: "Contain only letters (a-z, A-Z), numbers
/// (0-9), underscores (_), or dashes (-)" and "Contain 64 characters or fewer".
fn names() -> Vec<String> {
    vec![
        "metrics-set".to_string(),
        "Metrics_Set_2".to_string(),
        "9".to_string(),
        "a".repeat(64),
    ]
}

#[tokio::test]
async fn a_write_stores_the_value_it_carries_for_every_name_and_order() {
    let router = emulator();
    for name in names() {
        let (status, body) = create_configuration_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: expected 200, got {status} with {body}"
        );
        // A set nobody has written the flag on reports the member's documented
        // default: "The default value is false".
        let reported = reported_flag(&router, &name).await;
        assert!(
            !reported,
            "case fresh {name:?}: expected the documented default false, got {reported:?}"
        );

        // The request carries the state to hold, not a toggle: every value is
        // stored as given, in any order, and a repeat of the same value is the
        // same state.
        let writes = [true, true, false, false, true, false];
        for enabled in writes {
            let (status, body) = update_reputation_metrics(&router, &name, enabled).await;
            assert_eq!(
                status, 200,
                "case {name:?} write {enabled}: expected 200, got {status} with {body}"
            );
            assert!(
                body.contains("<UpdateConfigurationSetReputationMetricsEnabledResponse"),
                "case {name:?} write {enabled}: expected this operation's awsQuery envelope, \
                 got {body}"
            );
            let reported = reported_flag(&router, &name).await;
            assert_eq!(
                reported, enabled,
                "case {name:?} write {enabled}: expected the stored flag {enabled}, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn the_flag_is_held_per_configuration_set() {
    let router = emulator();
    let held = names();
    for name in &held {
        let (status, body) = create_configuration_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: expected 200, got {status} with {body}"
        );
    }

    // The sets are written one at a time, and after each write every set is
    // read: only the sets written to so far in this pass carry the new value,
    // and the rest still carry what the pass before left them — so a write
    // reaches exactly the set it names. The first pass starts from the default
    // (false), which is `!enabled` for its `true`.
    for enabled in [true, false, true] {
        for (written_at, written) in held.iter().enumerate() {
            let (status, body) = update_reputation_metrics(&router, written, enabled).await;
            assert_eq!(
                status, 200,
                "case {written:?} write {enabled}: expected 200, got {status} with {body}"
            );
            for (at, name) in held.iter().enumerate() {
                let reported = reported_flag(&router, name).await;
                let expected = if at <= written_at { enabled } else { !enabled };
                assert_eq!(
                    reported, expected,
                    "case {written:?} write {enabled}: expected {name:?} to report {expected}, \
                     got {reported}"
                );
            }
        }
    }

    // And the write touches the reputation-metrics flag only: the set's sending
    // state is the one its create left.
    for name in &held {
        let (_, sending_enabled) = described_reputation(&router, name).await;
        assert_eq!(
            sending_enabled.as_deref(),
            Some("true"),
            "case {name:?}: a reputation-metrics write leaves sending enabled, \
             got {sending_enabled:?}"
        );
    }
}

#[tokio::test]
async fn the_group_is_reported_only_when_the_request_asks_for_it() {
    let router = emulator();
    let (status, body) = create_configuration_set(&router, "metrics-set").await;
    assert_eq!(
        status, 200,
        "case create metrics-set: expected 200, got {status} with {body}"
    );
    let (status, body) = update_reputation_metrics(&router, "metrics-set", true).await;
    assert_eq!(
        status, 200,
        "case write metrics-set: expected 200, got {status} with {body}"
    );

    // Asked for, the stored flag is reported; not asked for, the whole group is
    // absent — the stored value is unchanged either way.
    for attributes in [
        vec![],
        vec!["trackingOptions"],
        vec!["reputationOptions"],
        vec!["trackingOptions", "reputationOptions"],
    ] {
        let (status, body) = describe_configuration_set(&router, "metrics-set", &attributes).await;
        assert_eq!(
            status, 200,
            "case describe asking {attributes:?}: expected 200, got {status} with {body}"
        );
        let asked = attributes.contains(&"reputationOptions");
        let reported = element(&body, "ReputationOptions")
            .as_deref()
            .and_then(|reputation| element(reputation, "ReputationMetricsEnabled"));
        let expected = asked.then(|| "true".to_string());
        assert_eq!(
            reported, expected,
            "case describe asking {attributes:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_write_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    // One set exists, so a refusal is about the name the request gave and not
    // about an empty store.
    let (status, body) = create_configuration_set(&router, "metrics-set").await;
    assert_eq!(
        status, 200,
        "case create metrics-set: expected 200, got {status} with {body}"
    );

    // Names spanning the same rule, none of them a set the account holds.
    let absent = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "b".repeat(64),
    ];
    for name in absent {
        for enabled in [true, false] {
            let (status, body) = update_reputation_metrics(&router, &name, enabled).await;
            assert_eq!(
                status, 400,
                "case absent {name:?} write {enabled}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                refusal(&body),
                (
                    Some("ConfigurationSetDoesNotExist".to_string()),
                    Some(format!("Configuration set <{name}> does not exist."))
                ),
                "case absent {name:?} write {enabled}: expected the not-found refusal, got {body}"
            );
        }

        // The refused writes stored nothing: the set is still not there...
        let (status, body) = describe_configuration_set(&router, &name, &["reputationOptions"]).await;
        assert_eq!(
            status, 400,
            "case describe absent {name:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{name}> does not exist."))
            ),
            "case describe absent {name:?}: expected the not-found refusal, got {body}"
        );
    }
    // ...and the set that does exist still carries the default.
    let reported = reported_flag(&router, "metrics-set").await;
    assert!(
        !reported,
        "case metrics-set after refused writes: expected the default false, got {reported:?}"
    );
}
