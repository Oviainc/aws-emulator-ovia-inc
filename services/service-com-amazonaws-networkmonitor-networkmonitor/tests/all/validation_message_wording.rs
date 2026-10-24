// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a refusal it already answers with the right status and
//! error code.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` already owns. The rules
//! under test:
//!
//! * a value outside a `@pattern` is refused in AWS's constraint-violation
//!   envelope, and the requirement it states quotes the expression a value must
//!   satisfy WITHOUT the `^`/`$` the model wraps it in to make the match total —
//!   so a client that lifts the expression out of the message and re-validates
//!   locally gets the one the service advertises. Every other requirement, and
//!   the envelope itself, is unchanged;
//! * the cross-member rule the model documents for `destinationPort` ("required
//!   only if the protocol is TCP") is refused with that rule's own sentence,
//!   whichever operation writes the probe record;
//! * the `aggregationPeriod` allow-list the model documents ("either 30 or 60")
//!   is refused with that rule's own sentence, which states the list and nothing
//!   of the value that missed it, whichever operation applies it;
//! * a refused request changes nothing, and the same request inside the rule is
//!   accepted and persists.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, get_monitor, name_of_length, probes_of, router,
    source_arn, string_at,
};

/// The clause AWS states a `@pattern` requirement with, and the one the
/// framework renders; what this suite pins is the expression that FOLLOWS it.
const PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: ";

/// `ProbeId`'s `@pattern` as the model declares it
/// (`smithy_query(query="shape",shape="ProbeId")` ->
/// `"^probe-[a-z0-9A-Z-]{21,64}$"`), carried by the `probeId` `@httpLabel` of
/// `GetProbe`, `DeleteProbe` and `UpdateProbe`.
const PROBE_ID_PATTERN: &str = r"^probe-[a-z0-9A-Z-]{21,64}$";

/// The expression a `ProbeId` refusal must state: [`PROBE_ID_PATTERN`] minus the
/// pair of anchors wrapping it.
const PROBE_ID_EXPRESSION: &str = r"probe-[a-z0-9A-Z-]{21,64}";

/// `ResourceName`'s `@pattern` as the model declares it
/// (`smithy_query(query="shape",shape="ResourceName")` ->
/// `"^[a-zA-Z0-9_-]+$"`, `@length(1..200)`), carried by the `monitorName`
/// `@httpLabel` of every monitor route.
const RESOURCE_NAME_PATTERN: &str = r"^[a-zA-Z0-9_-]+$";

/// The expression a `ResourceName` refusal must state.
const RESOURCE_NAME_EXPRESSION: &str = r"[a-zA-Z0-9_-]+";

/// `Arn`'s `@pattern` as the model declares it
/// (`smithy_query(query="shape",shape="Arn")` -> `"^arn:.*$"`,
/// `@length(20..2048)`), carried by the `resourceArn` `@httpLabel` of the two
/// tagging routes.
const ARN_PATTERN: &str = r"^arn:.*$";

/// The expression an `Arn` refusal must state.
const ARN_EXPRESSION: &str = r"arn:.*";

/// The sentence the service states for the documented cross-member rule
/// `ProbeInput$destinationPort` ("This is required only if the `protocol` is
/// `TCP`") / `UpdateProbeInput$protocol` ("If the protocol is TCP, then port is
/// also required").
const TCP_PORT_MESSAGE: &str = "Destination port must be provided for TCP protocol.";

/// The sentence the service states for the documented `aggregationPeriod`
/// allow-list (`UpdateMonitorInput$aggregationPeriod`: "This must be either 30
/// or 60"; `CreateMonitorInput$aggregationPeriod`: "Valid values are either 30
/// or 60").
const AGGREGATION_PERIOD_MESSAGE: &str = "The aggregation period must be one of [30, 60].";

/// One row of the `@pattern` table: the request to make, the member the clause
/// must name, the value it must echo, and the model's pattern beside the
/// expression the clause must state for it.
struct PatternCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<&'static str>,
    member: &'static str,
    value: String,
    /// The `@pattern` the model declares, which must NOT be stated.
    pattern: &'static str,
    /// That pattern minus the anchoring pair, which must be.
    expression: &'static str,
}

/// The `message` member of an error response, or `"<none>"`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

fn period_of(body: &Value) -> Option<i64> {
    body.get("aggregationPeriod").and_then(Value::as_i64)
}

async fn update_monitor(router: &MultiServiceRouter, name: &str, period: i64) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/monitors/{name}"),
        Some(&format!(r#"{{"aggregationPeriod":{period}}}"#)),
    )
    .await
}

async fn get_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

/// A probe body whose `sourceArn` names a subnet of this account.
fn probe_json(destination: &str, protocol: &str, extra: &str) -> String {
    format!(
        r#"{{"sourceArn":"{}","destination":"{destination}","protocol":"{protocol}"{extra}}}"#,
        source_arn()
    )
}

/// A `@pattern` refusal states the expression the model declares WITHOUT the
/// anchoring pair, in the framework's own envelope.
///
/// The table spans the class the rule covers: all three patterned members this
/// service binds to a request URI (`probeId`, `monitorName`, `resourceArn` —
/// three different expressions), each on more than one operation and HTTP
/// method, with values that miss the pattern in different ways (a wholly
/// different word, a too-short id, a character outside the admitted class).
/// The recorded case (`GetProbe` with `probeId=not-a-valid-probe-id`) is one
/// row.
#[tokio::test]
async fn a_pattern_requirement_is_stated_without_the_models_anchors() {
    let underscored_probe_id = format!("probe-{}_", "a".repeat(20));
    let cases: Vec<PatternCase> = vec![
        PatternCase {
            case: "the recorded case: GetProbe's probeId label",
            method: "GET",
            path: "/monitors/wording-host/probes/not-a-valid-probe-id".to_string(),
            body: None,
            member: "probeId",
            value: "not-a-valid-probe-id".to_string(),
            pattern: PROBE_ID_PATTERN,
            expression: PROBE_ID_EXPRESSION,
        },
        PatternCase {
            case: "DeleteProbe's probeId label, too short for the pattern",
            method: "DELETE",
            path: "/monitors/wording-host/probes/probe-short".to_string(),
            body: None,
            member: "probeId",
            value: "probe-short".to_string(),
            pattern: PROBE_ID_PATTERN,
            expression: PROBE_ID_EXPRESSION,
        },
        PatternCase {
            case: "UpdateProbe's probeId label, a character outside the class",
            method: "PATCH",
            path: format!("/monitors/wording-host/probes/{underscored_probe_id}"),
            body: Some(r#"{"packetSize":56}"#),
            member: "probeId",
            value: underscored_probe_id.clone(),
            pattern: PROBE_ID_PATTERN,
            expression: PROBE_ID_EXPRESSION,
        },
        PatternCase {
            case: "GetMonitor's monitorName label",
            method: "GET",
            path: "/monitors/bad.name".to_string(),
            body: None,
            member: "monitorName",
            value: "bad.name".to_string(),
            pattern: RESOURCE_NAME_PATTERN,
            expression: RESOURCE_NAME_EXPRESSION,
        },
        PatternCase {
            case: "DeleteMonitor's monitorName label",
            method: "DELETE",
            path: "/monitors/colon:name".to_string(),
            body: None,
            member: "monitorName",
            value: "colon:name".to_string(),
            pattern: RESOURCE_NAME_PATTERN,
            expression: RESOURCE_NAME_EXPRESSION,
        },
        PatternCase {
            case: "UpdateMonitor's monitorName label, with a body inside every bound",
            method: "PATCH",
            path: "/monitors/tilde~name".to_string(),
            body: Some(r#"{"aggregationPeriod":60}"#),
            member: "monitorName",
            value: "tilde~name".to_string(),
            pattern: RESOURCE_NAME_PATTERN,
            expression: RESOURCE_NAME_EXPRESSION,
        },
        PatternCase {
            case: "ListTagsForResource's resourceArn label",
            method: "GET",
            path: "/tags/not-an-arn-but-long-enough".to_string(),
            body: None,
            member: "resourceArn",
            value: "not-an-arn-but-long-enough".to_string(),
            pattern: ARN_PATTERN,
            expression: ARN_EXPRESSION,
        },
        PatternCase {
            case: "TagResource's resourceArn label",
            method: "POST",
            path: "/tags/also-not-an-arn-but-long".to_string(),
            body: Some(r#"{"tags":{"team":"net"}}"#),
            member: "resourceArn",
            value: "also-not-an-arn-but-long".to_string(),
            pattern: ARN_PATTERN,
            expression: ARN_EXPRESSION,
        },
    ];

    let router = router();
    let (status, created) = create_monitor(&router, r#"{"monitorName":"wording-host"}"#).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor expected 200, got {status} {created:?}"
    );

    for row in cases {
        let case = row.case;
        let (status, refused) = call(&router, row.method, &row.path, row.body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             {PATTERN_REQUIREMENT}{}",
            row.value, row.member, row.expression
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(row.pattern),
            "case {case:?}: the anchored model pattern {:?} must not be stated, got {message:?}",
            row.pattern
        );
    }
}

/// A requirement that is NOT a `@pattern` keeps the framework's own wording:
/// only the rendering of a pattern changes.
///
/// The rows cover the other constraint kinds this service binds to a request
/// URI — a `@length` maximum on a label, and both `@range` bounds on a query
/// parameter.
#[tokio::test]
async fn a_requirement_that_is_not_a_pattern_is_stated_as_the_framework_states_it() {
    let long_name = name_of_length(201);
    // (case, path, the requirement the clause must state)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "GetMonitor's monitorName label one past the @length maximum",
            format!("/monitors/{long_name}"),
            "Member must have length less than or equal to 200",
        ),
        (
            "ListMonitors' maxResults past the @range maximum",
            "/monitors?maxResults=26".to_string(),
            "Member must have value less than or equal to 25",
        ),
        (
            "ListMonitors' maxResults below the @range minimum",
            "/monitors?maxResults=0".to_string(),
            "Member must have value greater than or equal to 1",
        ),
    ];

    let router = router();
    for (case, path, requirement) in cases {
        let (status, refused) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let message = error_message(&refused);
        assert!(
            message.starts_with("1 validation error detected: Value "),
            "case {case:?}: expected the AWS envelope, got {message:?}"
        );
        assert!(
            message.ends_with(requirement),
            "case {case:?}: expected the clause to state {requirement:?}, got {message:?}"
        );
        assert!(
            !message.contains("regular expression pattern"),
            "case {case:?}: a non-pattern requirement must not state one, got {message:?}"
        );
    }
}

/// A probe asking for TCP without a `destinationPort` is refused with that
/// rule's own sentence, and nothing is written.
///
/// The table spans the class the rule covers: every operation that writes a
/// probe record (`CreateProbe`, `CreateMonitor`'s nested probes, `UpdateProbe`
/// switching a stored ICMP probe to TCP), both address families, a request with
/// and without `packetSize`, and a request carrying tags. The recorded case
/// (`CreateProbe` with an IPv4 destination and `packetSize` 56) is one row.
#[tokio::test]
async fn a_tcp_probe_without_a_destination_port_states_the_documented_rule() {
    let router = router();
    for name in ["tcp-host", "tcp-update-host"] {
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
        assert_eq!(
            status, 200,
            "setup CreateMonitor for {name:?} expected 200, got {status} {created:?}"
        );
    }
    // The probe the UpdateProbe row switches to TCP: stored ICMP, so it holds no
    // port of its own.
    let (status, probe) = create_probe(
        &router,
        "tcp-update-host",
        &format!(r#"{{"probe":{}}}"#, probe_json("10.0.0.9", "ICMP", "")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateProbe expected 200, got {status} {probe:?}"
    );
    let stored_probe_id = string_at(&probe, "probeId");

    // (case, method, path, body)
    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "the recorded case: CreateProbe, IPv4 destination, packetSize 56",
            "POST",
            "/monitors/tcp-host/probes".to_string(),
            format!(
                r#"{{"probe":{}}}"#,
                probe_json("192.168.63.63", "TCP", r#","packetSize":56"#)
            ),
        ),
        (
            "CreateProbe, IPv6 destination, no packetSize at all",
            "POST",
            "/monitors/tcp-host/probes".to_string(),
            format!(r#"{{"probe":{}}}"#, probe_json("2001:db8::1", "TCP", "")),
        ),
        (
            "CreateProbe at the modelled packetSize ceiling, with tags",
            "POST",
            "/monitors/tcp-host/probes".to_string(),
            format!(
                r#"{{"probe":{},"tags":{{"team":"net"}}}}"#,
                probe_json("198.51.100.7", "TCP", r#","packetSize":8500"#)
            ),
        ),
        (
            "CreateMonitor with a nested TCP probe and no destinationPort",
            "POST",
            "/monitors".to_string(),
            format!(
                r#"{{"monitorName":"tcp-nested","probes":[{}]}}"#,
                probe_json("203.0.113.4", "TCP", r#","packetSize":56"#)
            ),
        ),
        (
            "UpdateProbe switching a stored ICMP probe to TCP",
            "PATCH",
            format!("/monitors/tcp-update-host/probes/{stored_probe_id}"),
            r#"{"protocol":"TCP"}"#.to_string(),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, refused) = call(&router, method, &path, Some(&body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, TCP_PORT_MESSAGE,
            "case {case:?}: expected message {TCP_PORT_MESSAGE:?}, got {message:?}"
        );
    }

    // A refused probe is attached to nothing, and a refused create stores no
    // monitor at all.
    let (status, host) = get_monitor(&router, "tcp-host").await;
    assert_eq!(
        status, 200,
        "expected GetMonitor 200 for the host monitor, got {status} {host:?}"
    );
    assert!(
        probes_of(&host).is_empty(),
        "a refused CreateProbe must attach no probe, got {host:?}"
    );
    let (status, nested) = get_monitor(&router, "tcp-nested").await;
    assert_eq!(
        status, 404,
        "a refused CreateMonitor must store no monitor, got {status} {nested:?}"
    );
    // A refused update leaves the stored probe exactly as it was.
    let (status, unchanged) = get_probe(&router, "tcp-update-host", &stored_probe_id).await;
    assert_eq!(
        status, 200,
        "expected GetProbe 200 for the stored probe, got {status} {unchanged:?}"
    );
    assert_eq!(
        string_at(&unchanged, "protocol"),
        "ICMP",
        "a refused UpdateProbe must leave the stored protocol alone, got {unchanged:?}"
    );
    assert_eq!(
        unchanged.get("destinationPort"),
        None,
        "a refused UpdateProbe must store no port, got {unchanged:?}"
    );

    // The rule is about the PORT, not about the refusal: a protocol needing no
    // port, and TCP with one, are both accepted on that same monitor.
    for (case, probe) in [
        (
            "ICMP needs no port",
            probe_json("192.168.63.63", "ICMP", r#","packetSize":56"#),
        ),
        (
            "TCP with a port",
            probe_json("192.168.63.64", "TCP", r#","destinationPort":443,"packetSize":56"#),
        ),
    ] {
        let (status, created) =
            create_probe(&router, "tcp-host", &format!(r#"{{"probe":{probe}}}"#)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProbe 200, got {status} {created:?}"
        );
    }
    let (_, host) = get_monitor(&router, "tcp-host").await;
    assert_eq!(
        probes_of(&host).len(),
        2,
        "expected the two accepted probes to be attached, got {host:?}"
    );
}

/// An `aggregationPeriod` outside the documented allow-list is refused with that
/// rule's own sentence — which states the list and NOTHING of the value that
/// missed it — and the stored period is left as it was.
///
/// The table spans the class the rule covers: values inside the shape's own
/// `@range(min: 30)` but between, just past and far above the two documented
/// values, on both operations that apply the list (`UpdateMonitor`,
/// `CreateMonitor`). The recorded case (`UpdateMonitor` with 45) is one row.
#[tokio::test]
async fn an_aggregation_period_outside_the_documented_list_states_the_documented_rule() {
    // Values inside the modelled `@range(min: 30)` — so the handler's own
    // allow-list is what refuses them — and outside the documented pair.
    let refused_periods: &[(&str, i64)] = &[
        ("the recorded case: between the two documented values", 45),
        ("one past the documented floor", 31),
        ("one below the documented ceiling", 59),
        ("one past the documented ceiling", 61),
        ("well above both", 90),
        ("an hour", 3600),
    ];

    let router = router();
    let (status, created) = create_monitor(
        &router,
        r#"{"monitorName":"period-host","aggregationPeriod":30}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor expected 200, got {status} {created:?}"
    );

    for (case, period) in refused_periods {
        let (status, refused) = update_monitor(&router, "period-host", *period).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, AGGREGATION_PERIOD_MESSAGE,
            "case {case:?}: expected message {AGGREGATION_PERIOD_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(&period.to_string()),
            "case {case:?}: the rejected value {period} must not be echoed, got {message:?}"
        );
        // A refused update leaves the stored period as it was.
        let (status, got) = get_monitor(&router, "period-host").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            period_of(&got),
            Some(30),
            "case {case:?}: a refused update must leave the stored period alone, got {got:?}"
        );
    }

    // The same rule, and the same sentence, on the create path — and a refused
    // create stores no monitor.
    for (index, (case, period)) in refused_periods.iter().enumerate() {
        let name = format!("period-create-{index}");
        let (status, refused) = create_monitor(
            &router,
            &format!(r#"{{"monitorName":"{name}","aggregationPeriod":{period}}}"#),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateMonitor HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, AGGREGATION_PERIOD_MESSAGE,
            "case {case:?}: expected message {AGGREGATION_PERIOD_MESSAGE:?}, got {message:?}"
        );
        let (status, missing) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store nothing, got {status} {missing:?}"
        );
    }

    // A value INSIDE the list is accepted and persists.
    for period in [60, 30] {
        let (status, updated) = update_monitor(&router, "period-host", period).await;
        assert_eq!(
            status, 200,
            "case {period}: expected UpdateMonitor 200, got {status} {updated:?}"
        );
        assert_eq!(
            period_of(&updated),
            Some(period),
            "case {period}: expected the update to report {period}, got {updated:?}"
        );
        let (_, got) = get_monitor(&router, "period-host").await;
        assert_eq!(
            period_of(&got),
            Some(period),
            "case {period}: expected GetMonitor to report the stored {period}, got {got:?}"
        );
    }
}
