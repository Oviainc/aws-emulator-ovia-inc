// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `UpdateProbe`.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rules under test:
//!
//! * the route is a `PATCH` whose only required members are the two
//!   `@httpLabel`s, so an update replaces exactly the members the request names
//!   on the STORED probe and leaves every member it does not name as it was;
//!   what it answers is that stored record, so a later `GetProbe` and
//!   `GetMonitor` read back the same values;
//! * `addressFamily` stays a property of the destination the record holds, so it
//!   is re-derived whenever `destination` changes;
//! * the merged record must satisfy the same rules a create applies — the
//!   `destination` must be an IP literal, `destinationPort` is required when the
//!   `protocol` is `TCP` and must be at least `1` — and a refused update leaves
//!   the stored probe exactly as it was;
//! * `state` is the activate/deactivate toggle: only the states a CALLER may
//!   ask for are accepted, and a state the SERVICE assigns over a probe's
//!   lifecycle is refused with `ValidationException` and written nowhere;
//! * the TCP-requires-`destinationPort` rule is refused with the one sentence
//!   every path that applies it emits;
//! * `modifiedAt` moves only when a stored value actually changed (the operation
//!   is `@idempotent`), and `createdAt` never moves;
//! * a `monitorName` or `probeId` nothing is stored under is 404
//!   `ResourceNotFoundException`, and the update stores nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, get_monitor, probes_of, router, source_arn,
    string_at, ACCOUNT, PAST_ACTIVATION, PAST_TEARDOWN, REGION,
};

/// Every value the model's `ProbeState` enum admits
/// (`smithy_query(query="shape",shape="ProbeState")` ->
/// `PENDING | ACTIVE | INACTIVE | ERROR | DELETING | DELETED`), each marked
/// with whether a CALLER may put a probe into it through
/// `UpdateProbeInput$state` — the documented activate/deactivate toggle — or
/// whether the SERVICE assigns it over a probe's lifecycle (it provisions
/// through `PENDING`, records its own failure as `ERROR`, tears a probe down
/// through `DELETING` / `DELETED`).
const PROBE_STATES: &[(&str, bool)] = &[
    ("PENDING", false),
    ("ACTIVE", true),
    ("INACTIVE", true),
    ("ERROR", false),
    ("DELETING", false),
    ("DELETED", false),
];

/// The sentence AWS emits for the cross-member rule the model states on both
/// the create and the update path (`ProbeInput$destinationPort` "is required
/// only if the `protocol` is `TCP`"; `UpdateProbeInput$protocol` "If the
/// protocol is `TCP`, then `port` is also required").
const TCP_PORT_REQUIRED_MESSAGE: &str = "Destination port must be provided for TCP protocol.";

/// The members a probe record reports that an update can change, plus the two
/// derived ones every read must keep consistent with them.
const PROBE_MEMBERS: &[&str] = &[
    "destination",
    "destinationPort",
    "protocol",
    "packetSize",
    "addressFamily",
    "state",
];

async fn get_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

async fn update_probe(
    router: &MultiServiceRouter,
    name: &str,
    probe_id: &str,
    body: &str,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/monitors/{name}/probes/{probe_id}"),
        Some(body),
    )
    .await
}

/// The value a response reports for `member`, with an absent member read as
/// `Null` — so a row can state "this member is not reported".
fn member_of(body: &Value, member: &str) -> Value {
    body.get(member).cloned().unwrap_or(Value::Null)
}

/// The epoch-seconds a response reports for one timestamp member. Shared,
/// because what a record reports for `createdAt` / `modifiedAt` is asserted by
/// more than one suite (`probe_teardown` reads the same two members for the
/// delete path) and one reading of them keeps those suites from drifting.
pub(crate) fn timestamp(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("a probe record must report {member}, got {body:?}"))
}

/// Create a monitor holding one probe built from `probe` (a `probe` member body
/// with `SOURCE` standing in for a subnet ARN of this account) and answer the
/// monitor name beside the probe's minted id and created record.
async fn probe_fixture(
    router: &MultiServiceRouter,
    name: &str,
    probe: &str,
) -> (String, String, Value) {
    let (status, created) = create_monitor(router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor for {name:?} expected 200, got {status} {created:?}"
    );
    let body = format!(r#"{{"probe":{}}}"#, probe.replace("SOURCE", &source_arn()));
    let (status, created) = create_probe(router, name, &body).await;
    assert_eq!(
        status, 200,
        "setup CreateProbe for {name:?} expected 200, got {status} {created:?}"
    );
    let probe_id = string_at(&created, "probeId");
    (name.to_string(), probe_id, created)
}

/// An update replaces exactly the members it names and leaves the rest of the
/// stored record alone; what it answers is what every later read reports.
///
/// The table spans the class the rule covers: which members the request names
/// (one, several, all of them, none at all), their values at the published
/// bounds of `packetSize` (`@range(56..8500)`) and `destinationPort`
/// (documented floor `1`, modelled ceiling `65536`), both address families, both
/// protocols in each direction, and a stored probe that starts out with a port
/// and one that does not. The recorded case (an update naming only `packetSize`)
/// is one row.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_names() {
    // (case, created probe, PATCH body, the members the record must report after)
    let cases: &[(&str, &str, &str, Value)] = &[
        (
            "the recorded case: packetSize alone, on an ICMP probe",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}"#,
            r#"{"packetSize":1500}"#,
            json!({"destination": "10.0.0.1", "destinationPort": null, "protocol": "ICMP",
                   "packetSize": 1500, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "packetSize at the published range floor",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.2","protocol":"ICMP","packetSize":8500}"#,
            r#"{"packetSize":56}"#,
            json!({"destination": "10.0.0.2", "destinationPort": null, "protocol": "ICMP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "packetSize at the published range ceiling",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.3","protocol":"ICMP","packetSize":56}"#,
            r#"{"packetSize":8500}"#,
            json!({"destination": "10.0.0.3", "destinationPort": null, "protocol": "ICMP",
                   "packetSize": 8500, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "destination alone, IPv4 -> IPv6: addressFamily follows it",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.4","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":"2001:db8::1"}"#,
            json!({"destination": "2001:db8::1", "destinationPort": null, "protocol": "ICMP",
                   "packetSize": 56, "addressFamily": "IPV6", "state": "ACTIVE"}),
        ),
        (
            "destination alone, IPv6 -> IPv4: addressFamily follows it back",
            r#"{"sourceArn":"SOURCE","destination":"2001:db8::2","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":"198.51.100.7"}"#,
            json!({"destination": "198.51.100.7", "destinationPort": null, "protocol": "ICMP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "destination alone, IPv4 -> IPv4: the family is unchanged",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.5","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":"203.0.113.9"}"#,
            json!({"destination": "203.0.113.9", "destinationPort": null, "protocol": "ICMP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "destinationPort alone, on a TCP probe",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.6","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"destinationPort":8080}"#,
            json!({"destination": "10.0.0.6", "destinationPort": 8080, "protocol": "TCP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "destinationPort at the documented floor",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.7","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"destinationPort":1}"#,
            json!({"destination": "10.0.0.7", "destinationPort": 1, "protocol": "TCP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "destinationPort at the modelled range ceiling",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.8","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"destinationPort":65536}"#,
            json!({"destination": "10.0.0.8", "destinationPort": 65536, "protocol": "TCP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "protocol ICMP -> TCP, with the port the protocol then requires",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.9","protocol":"ICMP","packetSize":56}"#,
            r#"{"protocol":"TCP","destinationPort":22}"#,
            json!({"destination": "10.0.0.9", "destinationPort": 22, "protocol": "TCP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "protocol TCP -> ICMP alone: the stored port is not a member the request named",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.10","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"protocol":"ICMP"}"#,
            json!({"destination": "10.0.0.10", "destinationPort": 443, "protocol": "ICMP",
                   "packetSize": 56, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
        (
            "every updatable member at once",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.11","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":"2001:db8::beef","destinationPort":8443,"protocol":"TCP","packetSize":8500,"state":"INACTIVE"}"#,
            json!({"destination": "2001:db8::beef", "destinationPort": 8443, "protocol": "TCP",
                   "packetSize": 8500, "addressFamily": "IPV6", "state": "INACTIVE"}),
        ),
        (
            "state alone: nothing else about the record moves",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.12","protocol":"TCP","destinationPort":53,"packetSize":1500}"#,
            r#"{"state":"INACTIVE"}"#,
            json!({"destination": "10.0.0.12", "destinationPort": 53, "protocol": "TCP",
                   "packetSize": 1500, "addressFamily": "IPV4", "state": "INACTIVE"}),
        ),
        (
            "an update naming no member at all leaves the whole record",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.13","protocol":"TCP","destinationPort":53,"packetSize":1500}"#,
            r#"{}"#,
            json!({"destination": "10.0.0.13", "destinationPort": 53, "protocol": "TCP",
                   "packetSize": 1500, "addressFamily": "IPV4", "state": "ACTIVE"}),
        ),
    ];

    let router = router();
    for (index, (case, probe, update, expected)) in cases.iter().enumerate() {
        let (monitor, probe_id, created) =
            probe_fixture(&router, &format!("merge-case-{index}"), probe).await;
        // Past the activation span, so a row that does not name `state` is
        // compared against a settled `ACTIVE` probe rather than a transient one.
        std::thread::sleep(PAST_ACTIVATION);

        // Every row carries values the model admits, so each one is a 200 and
        // the refusals are the next table's business.
        let (status, updated) = update_probe(&router, &monitor, &probe_id, update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {updated:?}"
        );

        for member in PROBE_MEMBERS {
            assert_eq!(
                member_of(&updated, member),
                member_of(expected, member),
                "case {case:?}: expected {member} {:?}, got {:?} in {updated:?}",
                member_of(expected, member),
                member_of(&updated, member)
            );
        }
        // The identity of the probe is not something an update changes.
        for member in ["probeId", "probeArn", "sourceArn"] {
            assert_eq!(
                member_of(&updated, member),
                member_of(&created, member),
                "case {case:?}: an update must not change {member}, got {updated:?}"
            );
        }
        let expected_arn = format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:probe/{probe_id}");
        assert_eq!(
            string_at(&updated, "probeArn"),
            expected_arn,
            "case {case:?}: expected probeArn {expected_arn:?}, got {updated:?}"
        );

        // What was STORED is what every later read reports — the whole point of
        // the update.
        let (status, got) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProbe 200, got {status} {got:?}"
        );
        for member in PROBE_MEMBERS {
            assert_eq!(
                member_of(&got, member),
                member_of(expected, member),
                "case {case:?}: GetProbe must report {member} {:?}, got {got:?}",
                member_of(expected, member)
            );
        }
        let (_, monitor_body) = get_monitor(&router, &monitor).await;
        let listed = probes_of(&monitor_body)
            .into_iter()
            .find(|listed| string_at(listed, "probeId") == probe_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: GetMonitor must list probe {probe_id:?}, got {monitor_body:?}")
            });
        for member in PROBE_MEMBERS {
            assert_eq!(
                member_of(&listed, member),
                member_of(expected, member),
                "case {case:?}: GetMonitor must list {member} {:?}, got {listed:?}",
                member_of(expected, member)
            );
        }

        // `@idempotent`: the same update again answers the same record.
        let (status, again) = update_probe(&router, &monitor, &probe_id, update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated UpdateProbe 200, got {status} {again:?}"
        );
        assert_eq!(
            again, updated,
            "case {case:?}: a repeated update must answer the same record, got {again:?}"
        );
    }
}

/// `state` is the documented activate/deactivate toggle, so an update stores
/// only a state a CALLER may ask for: a request naming one of the lifecycle
/// values the SERVICE assigns itself is refused with the modelled
/// `ValidationException` (400) naming the rejected status, leaves the stored
/// probe exactly as it was, and does not stop a settable update on that same
/// probe from being accepted afterwards.
///
/// The table is the WHOLE modelled `ProbeState` catalog (`PROBE_STATES`), one
/// row per value, so it spans both halves of the rule rather than the one
/// recorded value: the two settable states are accepted and persist, the four
/// service-assigned ones are each refused.
#[tokio::test]
async fn only_the_states_a_caller_can_set_are_accepted() {
    let router = router();
    for (index, (requested, settable)) in PROBE_STATES.iter().enumerate() {
        let case = format!("state {requested} (settable by a caller: {settable})");
        let (monitor, probe_id, _) = probe_fixture(
            &router,
            &format!("state-case-{index}"),
            r#"{"sourceArn":"SOURCE","destination":"10.0.1.1","protocol":"ICMP","packetSize":56}"#,
        )
        .await;
        // Every row starts from the same settled `ACTIVE` probe.
        std::thread::sleep(PAST_ACTIVATION);
        let (_, settled) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            string_at(&settled, "state"),
            "ACTIVE",
            "case {case:?}: setup expected a settled ACTIVE probe, got {settled:?}"
        );
        let settled_modified_at = timestamp(&settled, "modifiedAt");

        let (status, answered) =
            update_probe(&router, &monitor, &probe_id, &format!(r#"{{"state":"{requested}"}}"#))
                .await;

        if *settable {
            assert_eq!(
                status, 200,
                "case {case:?}: a settable state expects HTTP 200, got {status} {answered:?}"
            );
            assert_eq!(
                string_at(&answered, "state"),
                *requested,
                "case {case:?}: expected state {requested}, got {answered:?}"
            );
            let (_, got) = get_probe(&router, &monitor, &probe_id).await;
            assert_eq!(
                string_at(&got, "state"),
                *requested,
                "case {case:?}: GetProbe must report the stored state {requested}, got {got:?}"
            );
            // A state the caller set owes the clock nothing, so it is still
            // there past the activation span.
            std::thread::sleep(PAST_ACTIVATION);
            let (_, later) = get_probe(&router, &monitor, &probe_id).await;
            assert_eq!(
                string_at(&later, "state"),
                *requested,
                "case {case:?}: past the activation span expected {requested}, got {later:?}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: a service-assigned state expects HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        assert_eq!(
            string_at(&answered, "message"),
            format!("{requested} is not a valid probe status."),
            "case {case:?}: expected the refusal to name the rejected status, got {answered:?}"
        );

        // The refused update wrote nothing: the probe is as it was.
        let (status, after) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the probe must still be there, got {status} {after:?}"
        );
        assert_eq!(
            string_at(&after, "state"),
            "ACTIVE",
            "case {case:?}: a refused state update leaves the stored state alone, got {after:?}"
        );
        assert!(
            (timestamp(&after, "modifiedAt") - settled_modified_at).abs() < f64::EPSILON,
            "case {case:?}: a refused update modified nothing, so modifiedAt must stay at \
             {settled_modified_at}, got {after:?}"
        );

        // Positive control: the refusal is about the value, not the probe — a
        // settable state is still accepted on it and converges.
        let (status, accepted) =
            update_probe(&router, &monitor, &probe_id, r#"{"state":"INACTIVE"}"#).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a settable state after the refusal expects HTTP 200, \
             got {status} {accepted:?}"
        );
        let (_, converged) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            string_at(&converged, "state"),
            "INACTIVE",
            "case {case:?}: GetProbe must converge to the settable state, got {converged:?}"
        );
    }
}

/// The TCP-requires-`destinationPort` rule is one rule, so every path that
/// applies it refuses with the one sentence AWS emits for it
/// (`TCP_PORT_REQUIRED_MESSAGE`) — and a request that supplies the port is
/// accepted on each of those paths.
///
/// The table spans the class: the rule holds wherever a probe record is
/// written, so it carries the two create paths (`CreateMonitor`'s inline
/// `probes` list and `CreateProbe`) beside the `UpdateProbe` path the recorded
/// case names, and the update row reaches it by naming `protocol` alone over a
/// stored probe that holds no port.
#[tokio::test]
async fn tcp_without_a_destination_port_is_refused_with_one_shared_message() {
    let router = router();
    let icmp = r#"{"sourceArn":"SOURCE","destination":"10.0.9.1","protocol":"ICMP","packetSize":56}"#;

    // CreateMonitor, with the probe in the request's inline `probes` list.
    let probes = |port: &str| {
        format!(
            r#"{{"monitorName":"tcp-message-monitor{suffix}","probes":[{{"sourceArn":"{source}",
                "destination":"10.0.9.2","protocol":"TCP"{port},"packetSize":56}}]}}"#,
            suffix = if port.is_empty() { "-refused" } else { "-ok" },
            source = source_arn(),
            port = port,
        )
    };
    let (status, refused) = create_monitor(&router, &probes("")).await;
    assert_eq!(
        status, 400,
        "case \"CreateMonitor with a TCP probe and no port\": expected HTTP 400, \
         got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"CreateMonitor with a TCP probe and no port\": expected ValidationException, \
         got {refused:?}"
    );
    assert_eq!(
        string_at(&refused, "message"),
        TCP_PORT_REQUIRED_MESSAGE,
        "case \"CreateMonitor with a TCP probe and no port\": expected the shared sentence, \
         got {refused:?}"
    );
    let (status, created) = create_monitor(&router, &probes(r#","destinationPort":443"#)).await;
    assert_eq!(
        status, 200,
        "case \"CreateMonitor with a TCP probe and a port\": expected HTTP 200, \
         got {status} {created:?}"
    );

    // CreateProbe, on a monitor that already exists.
    let (monitor, probe_id, _) = probe_fixture(&router, "tcp-message-probe", icmp).await;
    let tcp_probe = |port: &str| {
        format!(
            r#"{{"probe":{{"sourceArn":"{source}","destination":"10.0.9.3","protocol":"TCP"{port},
                "packetSize":56}}}}"#,
            source = source_arn(),
            port = port,
        )
    };
    let (status, refused) = create_probe(&router, &monitor, &tcp_probe("")).await;
    assert_eq!(
        status, 400,
        "case \"CreateProbe TCP with no port\": expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"CreateProbe TCP with no port\": expected ValidationException, got {refused:?}"
    );
    assert_eq!(
        string_at(&refused, "message"),
        TCP_PORT_REQUIRED_MESSAGE,
        "case \"CreateProbe TCP with no port\": expected the shared sentence, got {refused:?}"
    );
    let (status, created) =
        create_probe(&router, &monitor, &tcp_probe(r#","destinationPort":443"#)).await;
    assert_eq!(
        status, 200,
        "case \"CreateProbe TCP with a port\": expected HTTP 200, got {status} {created:?}"
    );

    // UpdateProbe, switching the stored ICMP probe (which holds no port) to TCP.
    let (status, refused) = update_probe(&router, &monitor, &probe_id, r#"{"protocol":"TCP"}"#).await;
    assert_eq!(
        status, 400,
        "case \"UpdateProbe to TCP with no port\": expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"UpdateProbe to TCP with no port\": expected ValidationException, got {refused:?}"
    );
    assert_eq!(
        string_at(&refused, "message"),
        TCP_PORT_REQUIRED_MESSAGE,
        "case \"UpdateProbe to TCP with no port\": expected the shared sentence, got {refused:?}"
    );
    // The refused update left the probe's ICMP configuration untouched.
    let (_, after) = get_probe(&router, &monitor, &probe_id).await;
    assert_eq!(
        string_at(&after, "protocol"),
        "ICMP",
        "case \"UpdateProbe to TCP with no port\": the stored protocol must stay ICMP, \
         got {after:?}"
    );
    assert_eq!(
        member_of(&after, "destinationPort"),
        Value::Null,
        "case \"UpdateProbe to TCP with no port\": the probe must still hold no port, \
         got {after:?}"
    );
    // Positive control: protocol and port together are accepted.
    let (status, updated) = update_probe(
        &router,
        &monitor,
        &probe_id,
        r#"{"protocol":"TCP","destinationPort":8080}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"UpdateProbe to TCP with a port\": expected HTTP 200, got {status} {updated:?}"
    );
    assert_eq!(
        member_of(&updated, "destinationPort"),
        json!(8080),
        "case \"UpdateProbe to TCP with a port\": expected the port stored, got {updated:?}"
    );
}

/// A probe deactivated and reactivated through `UpdateProbe` reports each state
/// it was put into, and `modifiedAt` moves with every change.
#[tokio::test]
async fn a_probe_can_be_deactivated_and_reactivated() {
    let router = router();
    let (monitor, probe_id, created) = probe_fixture(
        &router,
        "deactivate-reactivate",
        r#"{"sourceArn":"SOURCE","destination":"10.0.2.1","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
    )
    .await;
    let created_at = timestamp(&created, "createdAt");

    let mut previous = timestamp(&created, "modifiedAt");
    for (case, requested) in [
        ("deactivate", "INACTIVE"),
        ("reactivate", "ACTIVE"),
        ("deactivate again", "INACTIVE"),
    ] {
        let (status, updated) =
            update_probe(&router, &monitor, &probe_id, &format!(r#"{{"state":"{requested}"}}"#))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "state"),
            requested,
            "case {case:?}: expected state {requested}, got {updated:?}"
        );
        let modified_at = timestamp(&updated, "modifiedAt");
        assert!(
            modified_at > previous,
            "case {case:?}: the state changed, so modifiedAt must move past {previous}, \
             got {modified_at}"
        );
        assert!(
            (timestamp(&updated, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: createdAt never moves, got {updated:?}"
        );
        // The probe keeps everything the request did not name.
        assert_eq!(
            member_of(&updated, "destinationPort"),
            json!(443),
            "case {case:?}: an update naming only state keeps the port, got {updated:?}"
        );
        previous = modified_at;

        let (_, got) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            string_at(&got, "state"),
            requested,
            "case {case:?}: GetProbe must report the stored state, got {got:?}"
        );
    }
}

/// The merged record must satisfy the same rules a create applies, and a refused
/// update leaves the stored probe exactly as it was.
///
/// The table also pins WHICH refusal each row gets: a body value outside the
/// member's own modelled constraint is refused by the service's front door with
/// `BadRequestException`, while a value inside every modelled constraint that
/// breaks a rule the model states in prose reaches the handler and is refused
/// with the modelled `ValidationException`. It carries each published bound of
/// `packetSize` (`@range(56..8500)`) and `destinationPort`
/// (`@range(0..65536)`, documented floor `1`) and one past it.
#[tokio::test]
async fn an_update_is_refused_when_the_merged_record_breaks_a_rule() {
    // (case, created probe, PATCH body, expected status, expected error code)
    let cases: &[(&str, &str, &str, u16, &str)] = &[
        (
            "a destination that is not an IP literal",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.1","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":"example.internal"}"#,
            400,
            "ValidationException",
        ),
        (
            "a destination that is an IP literal with a port appended",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.2","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":"10.0.0.1:80"}"#,
            400,
            "ValidationException",
        ),
        (
            "a destination below the @length minimum",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.3","protocol":"ICMP","packetSize":56}"#,
            r#"{"destination":""}"#,
            400,
            "BadRequestException",
        ),
        (
            "protocol TCP on a probe that stores no port",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.4","protocol":"ICMP","packetSize":56}"#,
            r#"{"protocol":"TCP"}"#,
            400,
            "ValidationException",
        ),
        (
            "destinationPort 0: inside the modelled range, below the documented floor",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.5","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"destinationPort":0}"#,
            400,
            "ValidationException",
        ),
        (
            "destinationPort one past the modelled range ceiling",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.6","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"destinationPort":65537}"#,
            400,
            "BadRequestException",
        ),
        (
            "destinationPort negative",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.7","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            r#"{"destinationPort":-1}"#,
            400,
            "BadRequestException",
        ),
        (
            "packetSize one below the modelled range floor",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.8","protocol":"ICMP","packetSize":56}"#,
            r#"{"packetSize":55}"#,
            400,
            "BadRequestException",
        ),
        (
            "packetSize one past the modelled range ceiling",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.9","protocol":"ICMP","packetSize":56}"#,
            r#"{"packetSize":8501}"#,
            400,
            "BadRequestException",
        ),
        (
            "a protocol outside the modelled enum",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.10","protocol":"ICMP","packetSize":56}"#,
            r#"{"protocol":"SCTP"}"#,
            400,
            "BadRequestException",
        ),
        (
            "a state outside the modelled enum",
            r#"{"sourceArn":"SOURCE","destination":"10.0.3.11","protocol":"ICMP","packetSize":56}"#,
            r#"{"state":"RETIRED"}"#,
            400,
            "BadRequestException",
        ),
    ];

    let router = router();
    for (index, (case, probe, update, expected_status, expected_code)) in cases.iter().enumerate() {
        let (monitor, probe_id, created) =
            probe_fixture(&router, &format!("refusal-case-{index}"), probe).await;
        let (_, before) = get_probe(&router, &monitor, &probe_id).await;

        let (status, refused) = update_probe(&router, &monitor, &probe_id, update).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );

        // A refused update left the stored probe exactly as it was.
        let (status, after) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the probe must still be there, got {status} {after:?}"
        );
        for member in PROBE_MEMBERS {
            assert_eq!(
                member_of(&after, member),
                member_of(&before, member),
                "case {case:?}: a refused update must not change {member}, got {after:?}"
            );
        }
        assert!(
            (timestamp(&after, "modifiedAt") - timestamp(&created, "modifiedAt")).abs()
                < f64::EPSILON,
            "case {case:?}: a refused update modified nothing, so modifiedAt must stay put, \
             got {after:?}"
        );
    }
}

/// `modifiedAt` moves when the update changed a stored value, and stays put when
/// it changed none; `createdAt` never moves.
///
/// The table spans the class: a row per member an update can name, each sent
/// first as a value that differs from the stored one and then as the value the
/// record already holds, plus an update naming no member at all.
#[tokio::test]
async fn modified_at_moves_only_when_a_stored_value_changed() {
    // (case, PATCH body, whether the stored record changes)
    let cases: &[(&str, &str, bool)] = &[
        ("packetSize to a new value", r#"{"packetSize":1500}"#, true),
        ("packetSize to the value already stored", r#"{"packetSize":1500}"#, false),
        ("destination to a new address", r#"{"destination":"203.0.113.4"}"#, true),
        ("destination to the address already stored", r#"{"destination":"203.0.113.4"}"#, false),
        ("destinationPort to a new port", r#"{"destinationPort":8080}"#, true),
        ("destinationPort to the port already stored", r#"{"destinationPort":8080}"#, false),
        ("protocol to the one already stored", r#"{"protocol":"TCP"}"#, false),
        ("state to a new state", r#"{"state":"INACTIVE"}"#, true),
        ("state to the state already stored", r#"{"state":"INACTIVE"}"#, false),
        ("no member at all", r#"{}"#, false),
    ];

    let router = router();
    let (monitor, probe_id, created) = probe_fixture(
        &router,
        "modified-at",
        r#"{"sourceArn":"SOURCE","destination":"10.0.4.1","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
    )
    .await;
    let created_at = timestamp(&created, "createdAt");
    assert!(
        (timestamp(&created, "modifiedAt") - created_at).abs() < f64::EPSILON,
        "a freshly created probe has modifiedAt == createdAt, got {created:?}"
    );

    let mut modified_at = timestamp(&created, "modifiedAt");
    for (case, update, changes) in cases {
        let (status, updated) = update_probe(&router, &monitor, &probe_id, update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {updated:?}"
        );
        let now_modified_at = timestamp(&updated, "modifiedAt");
        if *changes {
            assert!(
                now_modified_at > modified_at,
                "case {case:?}: the update changed the record, so modifiedAt must move past \
                 {modified_at}, got {now_modified_at}"
            );
        } else {
            assert!(
                (now_modified_at - modified_at).abs() < f64::EPSILON,
                "case {case:?}: the update changed nothing, so modifiedAt must stay \
                 {modified_at}, got {now_modified_at}"
            );
        }
        assert!(
            (timestamp(&updated, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: createdAt never moves, got {updated:?}"
        );
        modified_at = now_modified_at;
    }
}

/// An update of a `monitorName` or a `probeId` nothing is stored under is 404
/// `ResourceNotFoundException`, and stores nothing.
///
/// The table spans a monitor that never existed, a stored monitor whose probe
/// id nothing minted, a probe whose teardown has finished, and a probe id of
/// another monitor — all at the shortest and longest spellings the modelled
/// `ProbeId` pattern (`^probe-[a-z0-9A-Z-]{21,64}$`) admits.
#[tokio::test]
async fn an_update_of_an_unknown_probe_is_not_found() {
    let router = router();
    let probe = r#"{"sourceArn":"SOURCE","destination":"10.0.5.1","protocol":"ICMP","packetSize":56}"#;
    let (present, present_probe, _) = probe_fixture(&router, "present-monitor", probe).await;
    let (other, other_probe, _) = probe_fixture(&router, "other-monitor", probe).await;

    // A probe of `present-monitor`, deleted again: its id is well-formed and was
    // minted, and once the teardown that delete started has run its course
    // nothing is stored under it any more. A probe is given up only once it has
    // left `PENDING`, so the clock carries it into service first.
    std::thread::sleep(PAST_ACTIVATION);
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/monitors/{present}/probes/{present_probe}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "setup DeleteProbe expected 200, got {deleted:?}");
    std::thread::sleep(PAST_TEARDOWN);

    let shortest = format!("probe-{}", "a".repeat(21));
    // 63 characters after the prefix, inside the pattern's 21..64.
    let longest = format!("probe-{}", "Z9-".repeat(21));
    // (case, monitorName, probeId)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a monitor nothing ever created",
            "absent-monitor".to_string(),
            shortest.clone(),
        ),
        (
            "a stored monitor, the shortest id the pattern admits",
            present.clone(),
            shortest,
        ),
        (
            "a stored monitor, the longest id the pattern admits",
            present.clone(),
            longest,
        ),
        (
            "the id of a probe whose teardown has finished",
            present.clone(),
            present_probe,
        ),
        (
            "a probe id of ANOTHER monitor",
            present.clone(),
            other_probe.clone(),
        ),
    ];

    for (case, monitor, probe_id) in cases {
        let (status, refused) =
            update_probe(&router, &monitor, &probe_id, r#"{"packetSize":1500}"#).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        // A refused update stores nothing under that id.
        let (status, got) = get_probe(&router, &monitor, &probe_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused update must store nothing, got {status} {got:?}"
        );
    }

    // The probe the refused calls named by id is still exactly where it was.
    let (status, intact) = get_probe(&router, &other, &other_probe).await;
    assert_eq!(
        status, 200,
        "the other monitor's probe must be untouched, got {status} {intact:?}"
    );
    assert_eq!(
        member_of(&intact, "packetSize"),
        json!(56),
        "an update of another monitor's name must not reach this probe, got {intact:?}"
    );
}
