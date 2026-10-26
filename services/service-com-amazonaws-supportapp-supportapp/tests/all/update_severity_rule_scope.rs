// SPDX-License-Identifier: CC0-1.0

//! What SCOPE each clause of the documented `notifyOnCaseSeverity` rule has on
//! `UpdateSlackChannelConfiguration`.
//!
//! The model words the two clauses of that rule over different scopes on an update
//! (`smithy_query(query="doc", shape="UpdateSlackChannelConfigurationRequest", member="notifyOnCaseSeverity",
//! model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")`):
//!
//! * `high` / `all`: "at least one of the following parameters must be true" — worded
//!   over the configuration, which with the member's own note ("If you don't specify
//!   these parameters in your request, the Amazon Web Services Support App uses the
//!   current values by default") is the record the update leaves behind.
//! * `none`: "any of the following parameters **that you specify in your request**
//!   must be false" — worded over the request, so a flag the request does not carry is
//!   not constrained by it at all and simply keeps its stored value.
//!
//! The input space of that rule is FINITE and small, so this suite sweeps it WHOLE
//! rather than sampling it: every stored fixture below, times every severity an update
//! can carry (including carrying none), times all twenty-seven ways the three
//! notification flags can be present-true, present-false or omitted. Each row's
//! expected answer is computed from the two clauses above by [`refusal_expected`],
//! never from the handler, and the row the issue recorded — severity to `none` with no
//! flag in the request, over a record that notifies on create-or-reopen — is one of
//! the rows the sweep reaches.
//!
//! Every assertion goes over the wire through the production router (the harness in
//! [`crate::slack_channel_configurations`]), and every accepted update is read back
//! through `ListSlackChannelConfigurations`, so a merge that is answered but not
//! persisted cannot pass.

use serde_json::{json, Value};

use crate::slack_channel_configurations::{role_arn, Emulator};

/// The three notification flags, in the order this API's prose lists them.
const FLAG_MEMBERS: [&str; 3] = [
    "notifyOnCreateOrReopenCase",
    "notifyOnAddCorrespondenceToCase",
    "notifyOnResolveCase",
];

/// Every way ONE flag can reach a handler from an update request: omitted (the member
/// is absent), explicitly `false`, explicitly `true`.
const FLAG_CHOICES: [Option<bool>; 3] = [None, Some(false), Some(true)];

/// Every severity an update can carry, plus carrying none at all — `notifyOnCaseSeverity`
/// is optional on the update request and its values are `all | high | none`
/// (`smithy_query(query="shape", shape="NotificationSeverityLevel")`).
const SEVERITY_CHOICES: [Option<&str>; 4] = [None, Some("none"), Some("high"), Some("all")];

/// A stored configuration an update is applied to.
#[derive(Debug, Clone, Copy)]
struct Fixture {
    label: &'static str,
    severity: &'static str,
    /// The three flags as created, sent explicitly so the stored record is unambiguous.
    flags: [bool; 3],
}

/// The stored records the sweep runs over: each severity level, and both a record with
/// one true flag and records with none or all of them true.
const FIXTURES: [Fixture; 4] = [
    Fixture {
        label: "high, notifying on create-or-reopen only",
        severity: "high",
        flags: [true, false, false],
    },
    Fixture {
        label: "high, notifying on resolve only",
        severity: "high",
        flags: [false, false, true],
    },
    Fixture {
        label: "all, notifying on everything",
        severity: "all",
        flags: [true, true, true],
    },
    Fixture {
        label: "none, notifying on nothing",
        severity: "none",
        flags: [false, false, false],
    },
];

impl Fixture {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "teamId": "T012ABCDEFG",
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": self.severity,
            "channelRoleArn": role_arn("SupportAppRole"),
        });
        for (member, flag) in FLAG_MEMBERS.into_iter().zip(self.flags) {
            body[member] = json!(flag);
        }
        body
    }
}

/// Whether the documented rule refuses this update, computed from the two clauses the
/// model words on the UPDATE request — the `all`/`high` clause over the record the
/// update would leave behind, the `none` clause over only the flags the request
/// carries.
fn refusal_expected(
    stored: &Fixture,
    severity: Option<&str>,
    requested: [Option<bool>; 3],
) -> bool {
    let effective_severity = severity.unwrap_or(stored.severity);
    let will_hold = merged_flags(stored, requested);
    match effective_severity {
        "high" | "all" => !will_hold.contains(&true),
        "none" => requested.contains(&Some(true)),
        _ => false,
    }
}

/// The flags the record holds after the update: what the request carries, else what is
/// stored.
fn merged_flags(stored: &Fixture, requested: [Option<bool>; 3]) -> [bool; 3] {
    let mut merged = stored.flags;
    for (slot, flag) in merged.iter_mut().zip(requested) {
        if let Some(value) = flag {
            *slot = value;
        }
    }
    merged
}

/// All twenty-seven ways the three flags can reach an update request.
fn flag_combinations() -> Vec<[Option<bool>; 3]> {
    let mut combinations = Vec::with_capacity(FLAG_CHOICES.len().pow(3));
    for create_or_reopen in FLAG_CHOICES {
        for add_correspondence in FLAG_CHOICES {
            for resolve in FLAG_CHOICES {
                combinations.push([create_or_reopen, add_correspondence, resolve]);
            }
        }
    }
    combinations
}

/// The update body for one row.
fn update_body(severity: Option<&str>, requested: [Option<bool>; 3]) -> Value {
    let mut body = json!({
        "teamId": "T012ABCDEFG",
        "channelId": "C01234A5BCD",
    });
    if let Some(severity) = severity {
        body["notifyOnCaseSeverity"] = json!(severity);
    }
    for (member, flag) in FLAG_MEMBERS.into_iter().zip(requested) {
        if let Some(value) = flag {
            body[member] = json!(value);
        }
    }
    body
}

/// Assert `reported` carries `severity` and `flags`.
fn assert_configuration(case: &str, reported: &Value, severity: &str, flags: [bool; 3]) {
    assert_eq!(
        reported.get("notifyOnCaseSeverity").and_then(Value::as_str),
        Some(severity),
        "case {case:?}: expected severity {severity:?}, got {reported:?}"
    );
    for (member, flag) in FLAG_MEMBERS.into_iter().zip(flags) {
        assert_eq!(
            reported.get(member).and_then(Value::as_bool),
            Some(flag),
            "case {case:?}: expected {member} to be {flag}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn each_clause_of_the_severity_rule_is_applied_over_its_documented_scope() {
    for stored in FIXTURES {
        for severity in SEVERITY_CHOICES {
            for requested in flag_combinations() {
                let case = format!(
                    "stored [{stored}], request severity {severity:?}, request flags {requested:?}",
                    stored = stored.label
                );
                let emulator = Emulator::new();
                let created = emulator
                    .create_in_registered_workspace(stored.create_body())
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: the fixture configuration must be created, got {} / {}",
                    created.status, created.body
                );

                let refused = refusal_expected(&stored, severity, requested);
                let reply = emulator.update(update_body(severity, requested)).await;
                let expected_status = if refused { 400 } else { 200 };
                assert_eq!(
                    reply.status, expected_status,
                    "case {case:?}: expected HTTP {expected_status}, got {} / {}",
                    reply.status, reply.body
                );

                let listed = emulator
                    .listed("T012ABCDEFG", "C01234A5BCD")
                    .await
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected the configuration to survive the update")
                    });
                if refused {
                    assert_eq!(
                        reply.error_type, "ValidationException",
                        "case {case:?}: expected ValidationException, got {:?}",
                        reply.error_type
                    );
                    // A refused update writes nothing: the stored record stands.
                    assert_configuration(&case, &listed, stored.severity, stored.flags);
                } else {
                    let expected_severity = severity.unwrap_or(stored.severity);
                    let expected_flags = merged_flags(&stored, requested);
                    assert_configuration(&case, &reply.body, expected_severity, expected_flags);
                    assert_configuration(&case, &listed, expected_severity, expected_flags);
                }
            }
        }
    }
}

#[tokio::test]
async fn an_update_to_none_carrying_no_flag_keeps_the_flags_it_does_not_mention() {
    // The row the issue recorded, spelled out: a record that notifies on
    // create-or-reopen at severity `high`, moved to `none` by a request that names no
    // notification flag. The `none` clause is worded over the flags the request
    // specifies, so it constrains nothing here — and the flags the request does not
    // mention keep their current values, as the member's note states.
    let emulator = Emulator::new();
    let created = emulator
        .create_in_registered_workspace(json!({
            "teamId": "T012ABCDEFG",
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": "high",
            "notifyOnCreateOrReopenCase": true,
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture configuration must be created, got {} / {}",
        created.status, created.body
    );

    let case = "severity to none with no notification flag in the request";
    let reply = emulator
        .update(json!({
            "teamId": "T012ABCDEFG",
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": "none",
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} / {}",
        reply.status, reply.body
    );
    assert_configuration(case, &reply.body, "none", [true, false, false]);

    let listed = emulator
        .listed("T012ABCDEFG", "C01234A5BCD")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to be listed"));
    assert_configuration(case, &listed, "none", [true, false, false]);

    // The same request again changes nothing further: the clause it is outside of
    // stays outside of it.
    let again = emulator
        .update(json!({
            "teamId": "T012ABCDEFG",
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": "none",
        }))
        .await;
    assert_eq!(
        again.status, 200,
        "case {case:?}: expected the repeated update to answer HTTP 200, got {} / {}",
        again.status, again.body
    );
    assert_configuration(case, &again.body, "none", [true, false, false]);
}
