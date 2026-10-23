// SPDX-License-Identifier: CC0-1.0

//! The monitor state lifecycle, the request-rejection classes, and the two
//! operations the monitor slice reads and updates through (`GetProbe`,
//! `UpdateMonitor`).
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` already owns. The rules
//! under test:
//!
//! * a monitor's `state` follows the probes it holds: no probe means nothing to
//!   aggregate, so it is `INACTIVE` with no transition due; a monitor that
//!   holds one is `PENDING` until the clock makes its activation due, then
//!   `ACTIVE`, and goes back to `INACTIVE` when its last probe is deleted;
//! * a value in the request BODY outside a constraint the model declares is
//!   refused at the service's front door with `BadRequestException`, while a
//!   value bound to the request URI (a `@httpLabel` segment, a `@httpQuery`
//!   parameter) outside its constraint is refused with the modelled
//!   `ValidationException`;
//! * a monitor this service does not hold is reported with the account and the
//!   `monitorName` the request named, a probe a monitor does not hold with its
//!   id beside them, and a duplicate `CreateMonitor` with the same two facts
//!   stated as a conflict;
//! * a probe's `sourceArn` must name a VPC subnet of the caller's account;
//! * `GetProbe` reports the stored probe. (`UpdateMonitor`'s own rules live in
//!   `tests/all/update_monitor.rs`, which owns that operation's catalog; this
//!   suite only covers it where the rules above reach it — the rejection class
//!   of its body and the message of its not-found.)

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, get_monitor, is_modelled_probe_id,
    name_of_length, probes_of, router, source_arn, string_at, ACCOUNT, PAST_ACTIVATION, REGION,
};

/// The `message` member of an error response, or `"<none>"`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// One probe body for `monitorName`-free use, with a `sourceArn` that names a
/// subnet of this account.
fn probe_json(destination: &str) -> String {
    format!(
        r#"{{"sourceArn":"{}","destination":"{destination}","protocol":"ICMP","packetSize":56}}"#,
        source_arn()
    )
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

/// A monitor's `state` is decided by the probes it holds, and the only
/// transition it is ever owed is the one the CLOCK makes due.
///
/// The table spans the probe count a create can ask for — none, one, several —
/// because that count is what the rule turns on.
#[tokio::test]
async fn a_monitors_state_follows_the_probes_it_holds() {
    // (case, probe count, state right after the create, state once the
    //  activation span has passed)
    let cases: &[(&str, usize, &str, &str)] = &[
        ("no probes: nothing to aggregate", 0, "INACTIVE", "INACTIVE"),
        ("one probe", 1, "PENDING", "ACTIVE"),
        ("two probes", 2, "PENDING", "ACTIVE"),
        ("three probes", 3, "PENDING", "ACTIVE"),
    ];

    let router = router();
    for (case, probe_count, initial, _settled) in cases {
        let name = format!("state-{probe_count}");
        let probes: Vec<String> = (0..*probe_count)
            .map(|i| probe_json(&format!("10.0.0.{}", i + 1)))
            .collect();
        let body = format!(
            r#"{{"monitorName":"{name}","probes":[{}]}}"#,
            probes.join(",")
        );
        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "state"),
            *initial,
            "case {case:?}: expected the created monitor in {initial}, got {created:?}"
        );
        // A caller reading straight after the call sees the same state the
        // create reported: the transition is owed to the clock, not to the read.
        let (_, got) = get_monitor(&router, &name).await;
        assert_eq!(
            string_at(&got, "state"),
            *initial,
            "case {case:?}: GetMonitor must report the stored {initial}, got {got:?}"
        );
        assert_eq!(
            probes_of(&got).len(),
            *probe_count,
            "case {case:?}: expected {probe_count} probes, got {got:?}"
        );
    }

    std::thread::sleep(PAST_ACTIVATION);

    for (case, probe_count, _, settled) in cases {
        let name = format!("state-{probe_count}");
        let (_, got) = get_monitor(&router, &name).await;
        assert_eq!(
            string_at(&got, "state"),
            *settled,
            "case {case:?}: once the activation span has passed, expected {settled}, got {got:?}"
        );
        for probe in probes_of(&got) {
            let expected = if *probe_count == 0 { "" } else { "ACTIVE" };
            assert_eq!(
                string_at(&probe, "state"),
                expected,
                "case {case:?}: expected every probe {expected}, got {probe:?}"
            );
        }
    }
}

/// Adding the first probe to an `INACTIVE` monitor gives it something to
/// aggregate, so the clock owes it `ACTIVE` again; deleting its last probe
/// takes that away and returns it to `INACTIVE`.
#[tokio::test]
async fn a_probe_added_to_an_idle_monitor_activates_it_and_its_deletion_idles_it_again() {
    let router = router();
    let (status, created) = create_monitor(&router, r#"{"monitorName":"idle-then-busy"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor -> {created:?}");
    assert_eq!(
        string_at(&created, "state"),
        "INACTIVE",
        "case \"before any probe\": expected INACTIVE, got {created:?}"
    );

    let (status, probe) = create_probe(
        &router,
        "idle-then-busy",
        &format!(r#"{{"probe":{}}}"#, probe_json("10.0.0.1")),
    )
    .await;
    assert_eq!(status, 200, "expected CreateProbe 200, got {probe:?}");
    let probe_id = string_at(&probe, "probeId");

    let (_, got) = get_monitor(&router, "idle-then-busy").await;
    assert_eq!(
        string_at(&got, "state"),
        "PENDING",
        "case \"straight after the first probe\": expected PENDING, got {got:?}"
    );

    std::thread::sleep(PAST_ACTIVATION);
    let (_, got) = get_monitor(&router, "idle-then-busy").await;
    assert_eq!(
        string_at(&got, "state"),
        "ACTIVE",
        "case \"once the activation span has passed\": expected ACTIVE, got {got:?}"
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/monitors/idle-then-busy/probes/{probe_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "expected DeleteProbe 200, got {deleted:?}");
    let (_, got) = get_monitor(&router, "idle-then-busy").await;
    assert_eq!(
        string_at(&got, "state"),
        "INACTIVE",
        "case \"after the last probe was deleted\": expected INACTIVE, got {got:?}"
    );
}

/// Where a value outside its modelled constraint travelled decides which error
/// refuses it: a BODY value is a front-door rejection (`BadRequestException`), a
/// value bound to the URI is the modelled `ValidationException`.
///
/// The table spans both locations across five operations and every constraint
/// kind the model uses on them — `@pattern`, `@length` (both bounds) and
/// `@range` (both bounds) — so the rule is the service's and not one handler's.
#[tokio::test]
async fn a_body_value_outside_its_constraint_is_refused_by_the_front_door() {
    let long_name = name_of_length(201);
    let probe_id = format!("probe-{}", "a".repeat(21));
    // (case, method, path, body, expected error code)
    let cases: Vec<(&str, &str, String, Option<String>, &str)> = vec![
        (
            "CreateMonitor: monitorName outside @pattern",
            "POST",
            "/monitors".to_string(),
            Some(r#"{"monitorName":"bad name!"}"#.to_string()),
            "BadRequestException",
        ),
        (
            "CreateMonitor: monitorName one past the @length maximum",
            "POST",
            "/monitors".to_string(),
            Some(format!(r#"{{"monitorName":"{long_name}"}}"#)),
            "BadRequestException",
        ),
        (
            "CreateMonitor: monitorName below the @length minimum",
            "POST",
            "/monitors".to_string(),
            Some(r#"{"monitorName":""}"#.to_string()),
            "BadRequestException",
        ),
        (
            "CreateMonitor: aggregationPeriod below the @range minimum",
            "POST",
            "/monitors".to_string(),
            Some(r#"{"monitorName":"front-door","aggregationPeriod":10}"#.to_string()),
            "BadRequestException",
        ),
        (
            "CreateMonitor: a nested probe's packetSize past the @range maximum",
            "POST",
            "/monitors".to_string(),
            Some(format!(
                r#"{{"monitorName":"front-door","probes":[{{"sourceArn":"{}",
                     "destination":"10.0.0.1","protocol":"ICMP","packetSize":9000}}]}}"#,
                source_arn()
            )),
            "BadRequestException",
        ),
        (
            "CreateProbe: probe.sourceArn below the @length minimum",
            "POST",
            "/monitors/front-door-host/probes".to_string(),
            Some(
                r#"{"probe":{"sourceArn":"arn:short","destination":"10.0.0.1",
                     "protocol":"ICMP","packetSize":56}}"#
                    .to_string(),
            ),
            "BadRequestException",
        ),
        (
            "CreateProbe: probe.protocol outside the modelled enum",
            "POST",
            "/monitors/front-door-host/probes".to_string(),
            Some(format!(
                r#"{{"probe":{{"sourceArn":"{}","destination":"10.0.0.1",
                     "protocol":"SCTP","packetSize":56}}}}"#,
                source_arn()
            )),
            "BadRequestException",
        ),
        (
            "UpdateMonitor: aggregationPeriod below the @range minimum",
            "PATCH",
            "/monitors/front-door-host".to_string(),
            Some(r#"{"aggregationPeriod":10}"#.to_string()),
            "BadRequestException",
        ),
        (
            "UpdateProbe: packetSize below the @range minimum",
            "PATCH",
            format!("/monitors/front-door-host/probes/{probe_id}"),
            Some(r#"{"packetSize":1}"#.to_string()),
            "BadRequestException",
        ),
        (
            "GetMonitor: the monitorName LABEL one past the @length maximum",
            "GET",
            format!("/monitors/{long_name}"),
            None,
            "ValidationException",
        ),
        (
            "GetMonitor: the monitorName LABEL outside @pattern",
            "GET",
            "/monitors/bad.name".to_string(),
            None,
            "ValidationException",
        ),
        (
            "DeleteMonitor: the monitorName LABEL one past the @length maximum",
            "DELETE",
            format!("/monitors/{long_name}"),
            None,
            "ValidationException",
        ),
        (
            "GetProbe: the probeId LABEL outside @pattern",
            "GET",
            "/monitors/front-door-host/probes/not-a-valid-probe-id".to_string(),
            None,
            "ValidationException",
        ),
        (
            "ListMonitors: the maxResults QUERY parameter past the @range maximum",
            "GET",
            "/monitors?maxResults=26".to_string(),
            None,
            "ValidationException",
        ),
        (
            "ListMonitors: the maxResults QUERY parameter below the @range minimum",
            "GET",
            "/monitors?maxResults=0".to_string(),
            None,
            "ValidationException",
        ),
    ];

    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"front-door-host"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    for (case, method, path, body, expected_code) in cases {
        let (status, refused) = call(&router, method, &path, body.as_deref()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );
    }

    // A refused body stores nothing.
    let (status, missing) = get_monitor(&router, "front-door").await;
    assert_eq!(
        status, 404,
        "a refused CreateMonitor must store no monitor, got {status} {missing:?}"
    );
    let (_, host) = get_monitor(&router, "front-door-host").await;
    assert!(
        probes_of(&host).is_empty(),
        "a refused CreateProbe must attach no probe, got {host:?}"
    );
    assert_eq!(
        host.get("aggregationPeriod").and_then(Value::as_i64),
        Some(60),
        "a refused UpdateMonitor must leave the stored period alone, got {host:?}"
    );
}

/// A resource this service does not hold is reported from the request and the
/// caller identity: the account the request was made under, and the name (and,
/// for a probe, the id) the request named.
///
/// The table spans every operation that looks a monitor up, and both resources
/// a lookup can miss.
#[tokio::test]
async fn a_missing_resource_is_reported_with_the_account_and_the_name_requested() {
    let absent_probe = format!("probe-{}", "b".repeat(21));
    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"present"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    // Three names, so the message is built from the request rather than from one
    // recorded spelling.
    for absent in ["absent", "never-created", &name_of_length(200)] {
        let expected =
            format!("No existing monitor found for account {ACCOUNT} and monitorName {absent}.");
        // (case, method, path, body)
        let cases: Vec<(&str, &str, String, Option<String>)> = vec![
            ("GetMonitor", "GET", format!("/monitors/{absent}"), None),
            (
                "DeleteMonitor",
                "DELETE",
                format!("/monitors/{absent}"),
                None,
            ),
            (
                "UpdateMonitor",
                "PATCH",
                format!("/monitors/{absent}"),
                Some(r#"{"aggregationPeriod":30}"#.to_string()),
            ),
            (
                "CreateProbe",
                "POST",
                format!("/monitors/{absent}/probes"),
                Some(format!(r#"{{"probe":{}}}"#, probe_json("10.0.0.1"))),
            ),
            (
                "GetProbe",
                "GET",
                format!("/monitors/{absent}/probes/{absent_probe}"),
                None,
            ),
            (
                "DeleteProbe",
                "DELETE",
                format!("/monitors/{absent}/probes/{absent_probe}"),
                None,
            ),
        ];
        for (case, method, path, body) in cases {
            let (status, refused) = call(&router, method, &path, body.as_deref()).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {absent:?}: expected HTTP 404, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?} on {absent:?}: expected ResourceNotFoundException, got {refused:?}"
            );
            assert_eq!(
                error_message(&refused),
                expected,
                "case {case:?} on {absent:?}: expected {expected:?}, got {refused:?}"
            );
        }
    }

    // A stored monitor that does not hold the probe names the probe too.
    for absent in [absent_probe.as_str(), &format!("probe-{}", "c".repeat(30))] {
        let expected = format!(
            "No existing probe {absent} found for account {ACCOUNT} and monitorName present."
        );
        for (case, method) in [("GetProbe", "GET"), ("DeleteProbe", "DELETE")] {
            let (status, refused) = call(
                &router,
                method,
                &format!("/monitors/present/probes/{absent}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?} on {absent:?}: expected HTTP 404, got {status} {refused:?}"
            );
            assert_eq!(
                error_message(&refused),
                expected,
                "case {case:?} on {absent:?}: expected {expected:?}, got {refused:?}"
            );
        }
    }
}

/// A `CreateMonitor` naming a monitor this service already holds is a conflict
/// stated from the same two facts — the account and the `monitorName` — and
/// names nothing the emulator generated.
#[tokio::test]
async fn a_duplicate_create_states_the_account_and_the_name() {
    let router = router();
    for name in ["dup-a", "dup_b", &name_of_length(120)] {
        let body = format!(r#"{{"monitorName":"{name}"}}"#);
        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: setup CreateMonitor -> {created:?}"
        );
        let (status, conflict) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected HTTP 409, got {status} {conflict:?}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {conflict:?}"
        );
        let expected =
            format!("Existing monitor found for account {ACCOUNT} and monitorName {name}.");
        assert_eq!(
            error_message(&conflict),
            expected,
            "case {name:?}: expected {expected:?}, got {conflict:?}"
        );
    }
}

/// A probe's `sourceArn` must name a VPC subnet of the caller's account: the
/// model types it as an `Arn` with `@pattern("^arn:.*$")` only, so the resource
/// it names is the service's to check, and a probe whose source is not a subnet
/// is refused with the modelled `ValidationException` and attached to nothing.
///
/// The table spans both EC2 id forms AWS publishes (8 and 17 characters), an ARN
/// of another service, two other EC2 resource types, a subnet of another
/// account, and an id too short to be either published form.
#[tokio::test]
async fn a_probes_source_must_name_a_subnet_of_the_callers_account() {
    // (case, sourceArn, accepted)
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "a subnet with a 17-character id",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0123456789abcdef0"),
            true,
        ),
        (
            "a subnet with an 8-character id",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-1a2b3c4d"),
            true,
        ),
        (
            "the recorded case: another service's ARN",
            format!("arn:aws:sns:{REGION}:{ACCOUNT}:not-a-subnet-topic"),
            false,
        ),
        (
            "an EC2 instance rather than a subnet",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef0"),
            false,
        ),
        (
            "an EC2 VPC rather than a subnet",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc/vpc-0123456789abcdef0"),
            false,
        ),
        (
            "a subnet of another account",
            format!("arn:aws:ec2:{REGION}:111122223333:subnet/subnet-0123456789abcdef0"),
            false,
        ),
        (
            "an id shorter than either published EC2 form",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-012"),
            false,
        ),
    ];

    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"subnet-host"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    for (index, (case, arn, accepted)) in cases.iter().enumerate() {
        let body = format!(
            r#"{{"probe":{{"sourceArn":"{arn}","destination":"10.0.{index}.1",
                 "protocol":"ICMP","packetSize":56}}}}"#
        );
        let (status, answered) = create_probe(&router, "subnet-host", &body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateProbe 200, got {status} {answered:?}"
            );
            assert_eq!(
                string_at(&answered, "sourceArn"),
                *arn,
                "case {case:?}: expected the stored sourceArn {arn:?}, got {answered:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateProbe 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        let resource = arn.rsplit(['/', ':']).next().unwrap_or(arn);
        let expected = format!("The specified subnet {resource} does not exist or could not be found.");
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );
    }

    // Exactly the two accepted rows are attached to the monitor.
    let (_, got) = get_monitor(&router, "subnet-host").await;
    assert_eq!(
        probes_of(&got).len(),
        2,
        "only a probe whose source names a subnet is attached, got {got:?}"
    );

    // The same rule holds for the probes a CreateMonitor carries, which go
    // through the one probe builder: the monitor is refused and stored nowhere.
    let body = format!(
        r#"{{"monitorName":"subnet-create","probes":[{{"sourceArn":"arn:aws:sns:{REGION}:{ACCOUNT}:not-a-subnet-topic",
             "destination":"10.1.0.1","protocol":"ICMP","packetSize":56}}]}}"#
    );
    let (status, refused) = create_monitor(&router, &body).await;
    assert_eq!(
        status, 400,
        "case \"CreateMonitor with a non-subnet probe source\": expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"CreateMonitor with a non-subnet probe source\": expected ValidationException, got {refused:?}"
    );
    let (status, missing) = get_monitor(&router, "subnet-create").await;
    assert_eq!(
        status, 404,
        "case \"CreateMonitor with a non-subnet probe source\": nothing is stored, got {status} {missing:?}"
    );
}

/// `GetProbe` reports the stored probe — every member the create answered with —
/// and 404s a well-formed id the monitor does not hold.
#[tokio::test]
async fn get_probe_reports_the_stored_probe() {
    // (case, probe JSON)
    let cases: &[(&str, &str)] = &[
        (
            "ICMP to an IPv4 address",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}"#,
        ),
        (
            "TCP to an IPv6 address with a port and tags",
            r#"{"sourceArn":"SOURCE","destination":"2001:db8::5","protocol":"TCP",
                "destinationPort":443,"packetSize":8500,"tags":{"role":"edge"}}"#,
        ),
    ];

    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"read-probes"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    for (case, probe) in cases {
        let body = format!(r#"{{"probe":{}}}"#, probe.replace("SOURCE", &source_arn()));
        let (status, created) = create_probe(&router, "read-probes", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProbe 200, got {status} {created:?}"
        );
        let probe_id = string_at(&created, "probeId");
        assert!(
            is_modelled_probe_id(&probe_id),
            "case {case:?}: expected a modelled probeId, got {probe_id:?}"
        );

        let (status, got) = get_probe(&router, "read-probes", &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProbe 200, got {status} {got:?}"
        );
        for member in [
            "probeId",
            "probeArn",
            "sourceArn",
            "destination",
            "destinationPort",
            "protocol",
            "packetSize",
            "addressFamily",
            "state",
            "tags",
        ] {
            assert_eq!(
                got.get(member),
                created.get(member),
                "case {case:?}: GetProbe's {member} must equal what CreateProbe stored, \
                 got {got:?} vs {created:?}"
            );
        }
        for member in ["createdAt", "modifiedAt"] {
            assert!(
                got.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: GetProbe must report {member}, got {got:?}"
            );
        }
    }

    // The probe states GetProbe reports are the states the monitor reports.
    std::thread::sleep(PAST_ACTIVATION);
    let (_, monitor) = get_monitor(&router, "read-probes").await;
    for listed in probes_of(&monitor) {
        let probe_id = string_at(&listed, "probeId");
        let (status, got) = get_probe(&router, "read-probes", &probe_id).await;
        assert_eq!(status, 200, "expected GetProbe 200, got {status} {got:?}");
        assert_eq!(
            string_at(&got, "state"),
            "ACTIVE",
            "case \"once the activation span has passed\": expected ACTIVE, got {got:?}"
        );
        assert_eq!(
            string_at(&got, "state"),
            string_at(&listed, "state"),
            "GetProbe and GetMonitor must report one state for one probe, got {got:?} vs {listed:?}"
        );
    }
}
