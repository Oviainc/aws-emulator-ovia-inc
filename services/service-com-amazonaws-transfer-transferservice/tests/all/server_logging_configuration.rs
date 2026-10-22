// SPDX-License-Identifier: CC0-1.0

//! Which logging members a described server CARRIES, and which it leaves out.
//!
//! ONE rule, asserted over a table of inputs rather than one recorded call: a server
//! reports a logging member only when it HAS that logging configuration, and an empty
//! value is the documented way of having none — so a server with a `LoggingRole` and no
//! structured destination describes WITHOUT `StructuredLogDestinations` (its logs go to
//! the service-managed group, which is exactly what the absent member says), and a
//! server with a structured destination and no role describes WITHOUT `LoggingRole`.
//!
//! The rows differ along which member is configured (neither, each alone, both), the
//! value configured (log-group ARNs in different regions, accounts and group names, and
//! one at `Arn`'s modelled minimum length), and the form of the empty value that means
//! "not configured" (member omitted, `LoggingRole` empty string, `StructuredLogDestinations`
//! empty list).
//!
//! The harness (router, request signing, `assert_members`) is the shared
//! [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{assert_members, Emulator};

/// A logging role ARN satisfying `NullableRole`'s modelled pattern
/// `^(|arn:.*role/\S+)$`.
const LOGGING_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-logs";

/// A second one, so a row that re-configures the role changes its VALUE too.
const OTHER_LOGGING_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-logs-2";

/// A log-group ARN in the form `StructuredLogDestinations`' model doc states:
/// `arn:partition:logs:region-name:amazon-account-id:log-group:log-group-name:*`.
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:mytestgroup:*";

/// `StructuredLogDestinations`' member targets `Arn`, modelled `len 20..1600` with
/// pattern `^arn:\S+$`; this is a value at the published MINIMUM length (20).
const SHORTEST_ARN: &str = "arn:aws:logs:1234567";

/// Every member of the described server this suite asserts, so a row states the whole
/// logging configuration and `null` means "must be absent".
fn logging_members(logging_role: Option<&str>, destinations: Option<&[&str]>) -> Value {
    json!({
        "LoggingRole": logging_role.map(Value::from).unwrap_or(Value::Null),
        "StructuredLogDestinations": destinations
            .map(|arns| Value::from(arns.to_vec()))
            .unwrap_or(Value::Null),
    })
}

/// A server's `LoggingRole` as `ListServers` reports it, or `Value::Null` when the
/// listed server does not carry the member.
async fn listed_logging_role(emulator: &Emulator, server_id: &str) -> Value {
    let answer = emulator.call("ListServers", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "ListServers: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("Servers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|server| server.get("ServerId").and_then(Value::as_str) == Some(server_id))
        .unwrap_or_else(|| panic!("ListServers did not list {server_id}: {}", answer.body))
        .get("LoggingRole")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Read a server back and assert its logging members, through `DescribeServer` and —
/// for `LoggingRole`, the one logging member `ListedServer` also carries — through
/// `ListServers`, so the two readers agree about what is configured.
async fn assert_described_logging(emulator: &Emulator, case: &str, server_id: &str, want: &Value) {
    let answer = emulator
        .call("DescribeServer", json!({"ServerId": server_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeServer 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_members(case, answer.get("Server"), want);

    let listed = listed_logging_role(emulator, server_id).await;
    let want_role = want.get("LoggingRole").cloned().unwrap_or(Value::Null);
    assert_eq!(
        listed, want_role,
        "case {case:?}: expected ListServers to report LoggingRole {want_role:?}, got {listed:?}"
    );
}

/// RULE: a described server carries a logging member only when the server HAS that
/// logging configuration; a member it was never given, and one given an empty value,
/// are absent from the response rather than reported at an empty default.
#[tokio::test]
async fn logging_members_are_reported_only_when_configured() {
    let emulator = Emulator::new();

    // (case, CreateServer request, expected logging members of the described server)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "neither member configured: logs go to the service-managed group",
            json!({}),
            logging_members(None, None),
        ),
        (
            "the legacy LoggingRole alone: no structured destination",
            json!({"LoggingRole": LOGGING_ROLE}),
            logging_members(Some(LOGGING_ROLE), None),
        ),
        (
            "a structured destination alone: no logging role",
            json!({"StructuredLogDestinations": [LOG_GROUP]}),
            logging_members(None, Some(&[LOG_GROUP])),
        ),
        (
            "a structured destination in another region, account and group",
            json!({
                "StructuredLogDestinations": [
                    "arn:aws:logs:eu-west-2:111122223333:log-group:/aws/transfer/other:*",
                ],
            }),
            logging_members(
                None,
                Some(&["arn:aws:logs:eu-west-2:111122223333:log-group:/aws/transfer/other:*"]),
            ),
        ),
        (
            "a destination ARN at the modelled minimum length",
            json!({"StructuredLogDestinations": [SHORTEST_ARN]}),
            logging_members(None, Some(&[SHORTEST_ARN])),
        ),
        (
            "both configured: each is reported from what the server holds",
            json!({
                "LoggingRole": OTHER_LOGGING_ROLE,
                "StructuredLogDestinations": [LOG_GROUP],
            }),
            logging_members(Some(OTHER_LOGGING_ROLE), Some(&[LOG_GROUP])),
        ),
        (
            // `NullableRole`'s pattern `^(|arn:.*role/\S+)$` admits the empty string
            // for exactly this purpose.
            "an empty LoggingRole configures no role",
            json!({"LoggingRole": ""}),
            logging_members(None, None),
        ),
        (
            // "You can clear it, and in effect turn off structured logging, by
            // providing an empty value for this parameter."
            "an empty StructuredLogDestinations configures no destination",
            json!({"StructuredLogDestinations": []}),
            logging_members(None, None),
        ),
        (
            "an empty value for each member together",
            json!({"LoggingRole": "", "StructuredLogDestinations": []}),
            logging_members(None, None),
        ),
    ];

    for (case, request, expected) in cases {
        let server_id = emulator.create_server(request).await;
        assert_described_logging(&emulator, case, &server_id, &expected).await;
    }
}

/// RULE (the same one, reached through `UpdateServer`): configuring a logging member
/// makes it appear, and clearing it with an empty value makes it ABSENT again — the
/// server goes back to the shape that says "this logging mode is not configured", and
/// the other logging member is untouched either way.
///
/// The rows are applied in sequence to ONE server, so each also asserts the previous
/// rows' configuration survived.
#[tokio::test]
async fn clearing_a_logging_member_makes_it_absent_again() {
    let emulator = Emulator::new();
    let server_id = emulator
        .create_server(json!({"LoggingRole": LOGGING_ROLE}))
        .await;

    // (case, UpdateServer request, expected logging members afterwards)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "created with the legacy role alone",
            json!({}),
            logging_members(Some(LOGGING_ROLE), None),
        ),
        (
            "adding a structured destination leaves the role alone",
            json!({"StructuredLogDestinations": [LOG_GROUP]}),
            logging_members(Some(LOGGING_ROLE), Some(&[LOG_GROUP])),
        ),
        (
            "replacing the destination keeps exactly one",
            json!({"StructuredLogDestinations": [SHORTEST_ARN]}),
            logging_members(Some(LOGGING_ROLE), Some(&[SHORTEST_ARN])),
        ),
        (
            "an empty list turns structured logging off: the member goes absent",
            json!({"StructuredLogDestinations": []}),
            logging_members(Some(LOGGING_ROLE), None),
        ),
        (
            "clearing it again changes nothing",
            json!({"StructuredLogDestinations": []}),
            logging_members(Some(LOGGING_ROLE), None),
        ),
        (
            "an empty LoggingRole clears the role: the member goes absent",
            json!({"LoggingRole": ""}),
            logging_members(None, None),
        ),
        (
            "configuring the structured destination alone",
            json!({"StructuredLogDestinations": [LOG_GROUP]}),
            logging_members(None, Some(&[LOG_GROUP])),
        ),
        (
            "configuring a role again while the destination stays",
            json!({"LoggingRole": OTHER_LOGGING_ROLE}),
            logging_members(Some(OTHER_LOGGING_ROLE), Some(&[LOG_GROUP])),
        ),
        (
            "a request that names no logging member changes neither",
            json!({"PreAuthenticationLoginBanner": "unrelated"}),
            logging_members(Some(OTHER_LOGGING_ROLE), Some(&[LOG_GROUP])),
        ),
    ];

    for (case, mut request, expected) in cases {
        request
            .as_object_mut()
            .expect("an update request object")
            .insert("ServerId".to_string(), json!(&server_id));
        let answer = emulator.call("UpdateServer", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateServer 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_described_logging(&emulator, case, &server_id, &expected).await;
    }
}
