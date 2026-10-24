// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a refusal its own code authors — one the model
//! declares a rule for only in prose, so the framework's boundary validator
//! cannot raise it.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` already owns.
//!
//! THE RULE: such a refusal is stated in AWS's constraint-violation
//! construction — `N validation error(s) detected: Value '<submitted>' at
//! '<member path>' failed to satisfy constraint: <requirement>` — built from the
//! request: the dotted path of the member the caller sent, that member's own
//! value quoted the way AWS quotes it (single quotes, capped like the boundary
//! validator caps an echo), and the requirement in the published template for
//! the rule the value broke:
//!
//! * an enumerated value set as BARE, SORTED values;
//! * a numeric bound as the bound;
//! * a rule the framework has no constraint class for (which cursors a listing
//!   vends, the format the model documents for an IP destination) as that rule.
//!
//! Nothing is rendered through Rust's `Debug`, which double-quotes a string and
//! is a language artefact no AWS service emits — so no message any case here
//! produces carries a `"`.
//!
//! What this suite does NOT cover is a refusal whose AWS sentence is known as a
//! whole rather than as a construction (`Destination port must be provided for
//! TCP protocol.`, `The aggregation period must be one of [30, 60].`, `<state>
//! is not a valid probe status.`): those keep their recorded sentence and are
//! pinned by `validation_message_wording` and `update_probe`.

use serde_json::Value;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, error_message, list_monitors, page_names,
    probe_json, router, string_at,
};

/// Every value the model's `MonitorState` enum admits
/// (`smithy_query(query="shape",shape="MonitorState")` ->
/// `PENDING | ACTIVE | INACTIVE | ERROR | DELETING`), which is the set
/// `ListMonitorsInput$state` filters on — the member targets a plain `string`,
/// so the service applies the enum itself. Listed here in the model's
/// DECLARATION order, which is deliberately not the order a refusal states.
const MONITOR_STATES: &[&str] = &["PENDING", "ACTIVE", "INACTIVE", "ERROR", "DELETING"];

/// The value set a refusal states: [`MONITOR_STATES`] sorted, which is how AWS
/// renders an enum value set.
const SORTED_MONITOR_STATES: &str = "ACTIVE, DELETING, ERROR, INACTIVE, PENDING";

/// The envelope AWS wraps a single constraint clause in.
const ONE_ERROR: &str = "1 validation error detected: ";

/// The text that separates a clause's subject from its requirement.
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// The framework's own cap on the value a clause echoes
/// (`aws_smithy_schema_server::constraints`' `MAX_ECHOED_VALUE`).
const MAX_ECHOED_VALUE: usize = 256;

/// The whole message AWS states for ONE violated constraint.
fn one_error(value: &str, member_path: &str, requirement: &str) -> String {
    format!("{ONE_ERROR}Value '{value}' at '{member_path}'{CONSTRAINT_MARKER}{requirement}")
}

/// The value a clause echoes for `submitted`: itself, or the framework's
/// character-boundary-safe prefix plus its elision.
fn echoed(submitted: &str) -> String {
    if submitted.len() <= MAX_ECHOED_VALUE {
        return submitted.to_string();
    }
    format!(
        "{}... [{} bytes]",
        &submitted[..MAX_ECHOED_VALUE],
        submitted.len()
    )
}

/// Assert the shared shape of every refusal this suite covers: 400,
/// `ValidationException`, the exact message, and no Rust `Debug` quoting.
fn assert_stated(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected message {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains('"'),
        "case {case:?}: a refusal must carry no Rust Debug quoting, got {message:?}"
    );
}

/// A `state` outside `MonitorState` is refused with AWS's enum-value-set clause,
/// over the whole set as the MODEL declares it and SORTED as AWS states it.
///
/// The table spans the ways a value can miss an enumerated set: a wholly
/// different word (the reported case, `FOO`), the wrong case of a member, a
/// prefix of one, a member with a suffix, a lookalike of one, and a value long
/// enough that the echo is elided. Every value the model DOES declare is
/// accepted on the same store, so the rule is about the set and not about
/// refusing.
#[tokio::test]
async fn a_state_outside_the_enumerated_set_is_stated_in_aws_constraint_construction() {
    let long_state = "X".repeat(300);
    let refused: Vec<(&str, String)> = vec![
        ("the reported case: a wholly different word", "FOO".to_string()),
        ("the wrong case of a declared value", "active".to_string()),
        ("a mixed case of a declared value", "Pending".to_string()),
        ("a prefix of a declared value", "ACTIV".to_string()),
        ("a declared value with a suffix", "ACTIVE2".to_string()),
        ("a lookalike of a declared value", "IN-ACTIVE".to_string()),
        ("a value past the framework's echo cap", long_state.clone()),
    ];

    let router = router();
    let (status, created) = create_monitor(&router, r#"{"monitorName":"state-filter-host"}"#).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor expected 200, got {status} {created:?}"
    );

    for (case, state) in &refused {
        let (status, body) = list_monitors(&router, &format!("?state={state}")).await;
        let expected = one_error(
            &echoed(state),
            "state",
            &format!("Member must satisfy enum value set: [{SORTED_MONITOR_STATES}]"),
        );
        assert_stated(case, status, &body, &expected);
        // The set is stated SORTED, never in the order the model declares it.
        let declared = MONITOR_STATES.join(", ");
        assert!(
            !error_message(&body).contains(&declared),
            "case {case:?}: the value set must not be stated in declaration order, got {:?}",
            error_message(&body)
        );
    }

    // Every value the model declares names a state a monitor can be in, so it
    // filters rather than refusing — including the ones no stored monitor is in.
    for state in MONITOR_STATES {
        let (status, body) = list_monitors(&router, &format!("?state={state}")).await;
        assert_eq!(
            status, 200,
            "case {state:?}: a declared MonitorState must filter, got {status} {body:?}"
        );
        let expected: Vec<String> = if *state == "INACTIVE" {
            vec!["state-filter-host".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            page_names(&body),
            expected,
            "case {state:?}: expected the monitors in that state to be {expected:?}, got {body:?}"
        );
    }
}

/// A `nextToken` this service never vended is refused with AWS's clause for the
/// rule a cursor has to satisfy — that a previous call of THIS listing vended
/// it — naming the member and the token the caller sent.
///
/// The rows span the ways a string can fail to be a cursor this service minted:
/// text that is not the token's encoding at all, text that is but carries
/// nothing this service wrote, the bare encoding of a stored `monitorName`
/// (which is what keeps a caller from hand-rolling a cursor out of a name it
/// read), and a token long enough that the echo is elided. A cursor the service
/// DID vend is accepted on the same store.
#[tokio::test]
async fn a_next_token_this_service_never_vended_is_stated_as_the_rule_it_broke() {
    let long_token = "z".repeat(300);
    let refused: Vec<(&str, String)> = vec![
        ("text that is not the token encoding", "notatoken".to_string()),
        (
            "the encoding of text nothing vended",
            "6e6f742d612d637572736f72".to_string(),
        ),
        (
            "the bare encoding of a stored monitorName",
            "6d6f6e2d62".to_string(),
        ),
        ("a token past the framework's echo cap", long_token.clone()),
    ];

    let router = router();
    for name in ["mon-a", "mon-b"] {
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
        assert_eq!(
            status, 200,
            "setup CreateMonitor {name} expected 200, got {status} {created:?}"
        );
    }

    for (case, token) in &refused {
        let (status, body) = list_monitors(&router, &format!("?nextToken={token}")).await;
        let expected = one_error(
            &echoed(token),
            "nextToken",
            "Member must be a nextToken returned by a previous ListMonitors call",
        );
        assert_stated(case, status, &body, &expected);
    }

    // A cursor this listing vended resumes it, so the rule is about where the
    // token came from and not about refusing every token.
    let (status, first) = list_monitors(&router, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "expected the first page to be served, got {status} {first:?}"
    );
    let vended = string_at(&first, "nextToken");
    let (status, resumed) = list_monitors(&router, &format!("?nextToken={vended}")).await;
    assert_eq!(
        status, 200,
        "a vended cursor must resume the listing, got {status} {resumed:?}"
    );
    assert_eq!(
        page_names(&resumed),
        vec!["mon-b".to_string()],
        "expected the resume to report the monitors still owed, got {resumed:?}"
    );
}

/// The two rules the model documents for a probe in prose — the `destination`
/// must be an IP address, and `destinationPort` has a floor of `1` the shape's
/// own `@range(min: 0)` admits past — are stated in the same construction, and
/// the clause names the member WHERE THE CALLER SENT IT.
///
/// The table spans the three operations that write a probe record and therefore
/// the three places a probe's members sit in a request (`CreateProbe`'s `probe`,
/// one element of `CreateMonitor`'s `probes` list at two different positions,
/// and `UpdateProbe`'s top-level members), both rules, and several ways a
/// destination can fail to be an IP literal (a host name, a truncated dotted
/// quad, an octet out of range, a CIDR, an over-long IPv6).
#[tokio::test]
async fn a_probe_member_outside_its_documented_rule_names_the_path_the_caller_sent() {
    const DESTINATION_REQUIREMENT: &str = "Member must be an IPv4 or IPv6 address";
    const PORT_FLOOR_REQUIREMENT: &str = "Member must have value greater than or equal to 1";

    let router = router();
    let (status, created) = create_monitor(&router, r#"{"monitorName":"probe-rules-host"}"#).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor expected 200, got {status} {created:?}"
    );
    // The probe `UpdateProbe` is driven against: stored ICMP, so it holds no
    // port of its own.
    let (status, stored) = create_probe(
        &router,
        "probe-rules-host",
        &format!(r#"{{"probe":{}}}"#, probe_json("10.0.0.1", "ICMP", "")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateProbe expected 200, got {status} {stored:?}"
    );
    let probe_id = string_at(&stored, "probeId");

    // (case, method, path, body, the value the clause echoes, the member path it
    // names, the requirement it states)
    let cases: Vec<(&str, &str, String, String, String, String, &str)> = vec![
        (
            "CreateProbe with a host name for a destination",
            "POST",
            "/monitors/probe-rules-host/probes".to_string(),
            format!(
                r#"{{"probe":{}}}"#,
                probe_json("example.internal", "ICMP", "")
            ),
            "example.internal".to_string(),
            "probe.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateProbe with a truncated dotted quad",
            "POST",
            "/monitors/probe-rules-host/probes".to_string(),
            format!(r#"{{"probe":{}}}"#, probe_json("10.0.0", "ICMP", "")),
            "10.0.0".to_string(),
            "probe.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateProbe with an octet out of range",
            "POST",
            "/monitors/probe-rules-host/probes".to_string(),
            format!(r#"{{"probe":{}}}"#, probe_json("10.0.0.256", "ICMP", "")),
            "10.0.0.256".to_string(),
            "probe.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateProbe with a CIDR block rather than an address",
            "POST",
            "/monitors/probe-rules-host/probes".to_string(),
            format!(r#"{{"probe":{}}}"#, probe_json("10.0.0.0/24", "ICMP", "")),
            "10.0.0.0/24".to_string(),
            "probe.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateProbe with an over-long IPv6 literal",
            "POST",
            "/monitors/probe-rules-host/probes".to_string(),
            format!(
                r#"{{"probe":{}}}"#,
                probe_json("2001:db8::1::2", "ICMP", "")
            ),
            "2001:db8::1::2".to_string(),
            "probe.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateMonitor's first nested probe",
            "POST",
            "/monitors".to_string(),
            format!(
                r#"{{"monitorName":"nested-first","probes":[{}]}}"#,
                probe_json("not-an-address", "ICMP", "")
            ),
            "not-an-address".to_string(),
            "probes.1.member.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateMonitor's second nested probe",
            "POST",
            "/monitors".to_string(),
            format!(
                r#"{{"monitorName":"nested-second","probes":[{},{}]}}"#,
                probe_json("192.0.2.10", "ICMP", ""),
                probe_json("still-not-an-address", "ICMP", "")
            ),
            "still-not-an-address".to_string(),
            "probes.2.member.destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "UpdateProbe's top-level destination",
            "PATCH",
            format!("/monitors/probe-rules-host/probes/{probe_id}"),
            r#"{"destination":"not-an-address"}"#.to_string(),
            "not-an-address".to_string(),
            "destination".to_string(),
            DESTINATION_REQUIREMENT,
        ),
        (
            "CreateProbe below the documented destinationPort floor",
            "POST",
            "/monitors/probe-rules-host/probes".to_string(),
            format!(
                r#"{{"probe":{}}}"#,
                probe_json("192.0.2.20", "TCP", r#","destinationPort":0"#)
            ),
            "0".to_string(),
            "probe.destinationPort".to_string(),
            PORT_FLOOR_REQUIREMENT,
        ),
        (
            "CreateMonitor's nested probe below the floor",
            "POST",
            "/monitors".to_string(),
            format!(
                r#"{{"monitorName":"nested-port","probes":[{}]}}"#,
                probe_json("192.0.2.21", "TCP", r#","destinationPort":0"#)
            ),
            "0".to_string(),
            "probes.1.member.destinationPort".to_string(),
            PORT_FLOOR_REQUIREMENT,
        ),
        (
            "UpdateProbe below the documented destinationPort floor",
            "PATCH",
            format!("/monitors/probe-rules-host/probes/{probe_id}"),
            r#"{"destinationPort":0}"#.to_string(),
            "0".to_string(),
            "destinationPort".to_string(),
            PORT_FLOOR_REQUIREMENT,
        ),
    ];

    for (case, method, path, body, value, member_path, requirement) in cases {
        let (status, refused) = call(&router, method, &path, Some(&body)).await;
        let expected = one_error(&value, &member_path, requirement);
        assert_stated(case, status, &refused, &expected);
    }

    // A refused probe wrote nothing: the host monitor still holds exactly the
    // probe the setup attached, and no refused create stored a monitor.
    let (status, host) = call(&router, "GET", "/monitors/probe-rules-host", None).await;
    assert_eq!(
        status, 200,
        "expected GetMonitor 200 for the host monitor, got {status} {host:?}"
    );
    let (status, probe) = call(
        &router,
        "GET",
        &format!("/monitors/probe-rules-host/probes/{probe_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the stored probe to still be there, got {status} {probe:?}"
    );
    assert_eq!(
        string_at(&probe, "destination"),
        "10.0.0.1",
        "a refused update must leave the stored destination alone, got {probe:?}"
    );
    assert_eq!(
        probe.get("destinationPort"),
        None,
        "a refused update must store no port, got {probe:?}"
    );
    for name in ["nested-first", "nested-second", "nested-port"] {
        let (status, missing) = call(&router, "GET", &format!("/monitors/{name}"), None).await;
        assert_eq!(
            status, 404,
            "a refused CreateMonitor must store no monitor {name}, got {status} {missing:?}"
        );
    }
}

/// A refusal this service AUTHORS is stated in the same construction as one the
/// framework's boundary validator raises: same envelope, same clause shape, same
/// separator — only the requirement differs, because only the rule does.
///
/// The rows pair each authored refusal with a refusal of a MODELLED constraint
/// on the same operation (`maxResults` outside its `@range`, `monitorName`
/// outside its `@pattern`), which is the one the framework words.
#[tokio::test]
async fn an_authored_refusal_is_built_like_a_modelled_one() {
    let router = router();

    // (case, method, path, body)
    let authored: Vec<(&str, &str, String, Option<String>)> = vec![
        (
            "ListMonitors' state filter",
            "GET",
            "/monitors?state=FOO".to_string(),
            None,
        ),
        (
            "ListMonitors' nextToken",
            "GET",
            "/monitors?nextToken=notatoken".to_string(),
            None,
        ),
        (
            "CreateProbe's probe.destination",
            "POST",
            "/monitors/shape-host/probes".to_string(),
            Some(format!(
                r#"{{"probe":{}}}"#,
                probe_json("not-an-address", "ICMP", "")
            )),
        ),
    ];
    let modelled: Vec<(&str, &str, String, Option<String>)> = vec![
        (
            "ListMonitors' maxResults past its @range",
            "GET",
            "/monitors?maxResults=26".to_string(),
            None,
        ),
        (
            "GetMonitor's monitorName outside its @pattern",
            "GET",
            "/monitors/bad.name".to_string(),
            None,
        ),
    ];

    let (status, created) = create_monitor(&router, r#"{"monitorName":"shape-host"}"#).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor expected 200, got {status} {created:?}"
    );

    for (kind, rows) in [("authored", authored), ("modelled", modelled)] {
        for (case, method, path, body) in rows {
            let (status, refused) = call(&router, method, &path, body.as_deref()).await;
            assert_eq!(
                status, 400,
                "case {case:?} ({kind}): expected HTTP 400, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?} ({kind}): expected ValidationException, got {refused:?}"
            );
            let message = error_message(&refused);
            assert!(
                message.starts_with(&format!("{ONE_ERROR}Value '")),
                "case {case:?} ({kind}): expected AWS's one-error envelope, got {message:?}"
            );
            let clause = message
                .split_once(CONSTRAINT_MARKER)
                .map(|(subject, requirement)| (subject.to_string(), requirement.to_string()));
            let Some((subject, requirement)) = clause else {
                panic!("case {case:?} ({kind}): expected the constraint marker in {message:?}");
            };
            assert!(
                subject.ends_with('\'') && subject.matches(" at '").count() == 1,
                "case {case:?} ({kind}): expected a `Value '<v>' at '<path>'` subject, got \
                 {subject:?}"
            );
            assert!(
                requirement.starts_with("Member must "),
                "case {case:?} ({kind}): expected a `Member must …` requirement, got \
                 {requirement:?}"
            );
            assert!(
                !message.contains('"'),
                "case {case:?} ({kind}): no refusal carries Rust Debug quoting, got {message:?}"
            );
        }
    }
}
