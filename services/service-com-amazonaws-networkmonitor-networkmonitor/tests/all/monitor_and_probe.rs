// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the CloudWatch Network Monitor monitor + probe slice.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees — not a handler called
//! directly. The rules under test:
//!
//! * a monitor is a stored resource: a create is visible to every later read of
//!   the same name, a probe added to it is listed by `GetMonitor`, and a delete
//!   makes every read of that name 404 `ResourceNotFoundException`;
//! * `aggregationPeriod` is one of the two values the model documents, and `60`
//!   when the request names none;
//! * a monitor's `state` follows the probes it holds: `INACTIVE` with none,
//!   `PENDING` until the clock makes its activation due, then `ACTIVE`;
//! * a probe's `addressFamily` is derived from its `destination`, and the
//!   documented `destinationPort` rules (required for TCP, at least `1`) are
//!   enforced where the model's own constraints cannot express them;
//! * a `probeId` matches the modelled `ProbeId` pattern and is unique;
//! * `ListMonitors` pages in `monitorName` order, honours `maxResults`, round
//!   trips its opaque `nextToken`, filters on `state`, and omits `nextToken` on
//!   the final page;
//! * the `PENDING` -> `ACTIVE` transition is owed to the clock, not to the read
//!   that observes it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` naming this service's signing name
/// (`networkmonitor`) — how the router picks the service — and the region in its
/// credential scope, which is the region every ARN the service mints carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/networkmonitor/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The region in [`AUTHORIZATION`]'s credential scope.
pub(crate) const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// Longer than the monitor's activation span, so a read after this sleep
/// observes the `PENDING` -> `ACTIVE` transition the clock has already made due.
///
/// A blocking sleep rather than `tokio::time::sleep`: the workspace's `tokio`
/// dependency enables only `macros`, `rt` and `rt-multi-thread`, and the root
/// `Cargo.toml` that declares it is not editable from here, so the `time`
/// feature is not available. The handlers read the wall clock, so the wait has
/// to be real either way.
pub(crate) const PAST_ACTIVATION: Duration = Duration::from_millis(900);

/// The alphabet the `ResourceName` pattern admits: `^[a-zA-Z0-9_-]+$`.
const NAME_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

/// A subnet ARN to use as a probe's `sourceArn`; the model types it as `Arn`
/// with `@length(20..2048)` and `@pattern("^arn:.*$")`.
pub(crate) fn source_arn() -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0123456789abcdef0")
}

pub(crate) fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_networkmonitor_networkmonitor::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null`
/// for an empty body).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response (`__type`), or `"<none>"`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

pub(crate) fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) async fn create_monitor(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "POST", "/monitors", Some(body)).await
}

pub(crate) async fn get_monitor(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &format!("/monitors/{name}"), None).await
}

pub(crate) async fn create_probe(router: &MultiServiceRouter, name: &str, body: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/monitors/{name}/probes"),
        Some(body),
    )
    .await
}

pub(crate) async fn list_monitors(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/monitors{query}"), None).await
}

/// The `monitorName`s a `ListMonitors` page reports, in the order it reports
/// them.
pub(crate) fn page_names(body: &Value) -> Vec<String> {
    body.get("monitors")
        .and_then(Value::as_array)
        .map(|monitors| {
            monitors
                .iter()
                .map(|monitor| string_at(monitor, "monitorName"))
                .collect()
        })
        .unwrap_or_default()
}

/// The probes a `GetMonitor` body reports.
pub(crate) fn probes_of(body: &Value) -> Vec<Value> {
    body.get("probes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Whether `id` satisfies the model's `ProbeId` pattern,
/// `^probe-[a-z0-9A-Z-]{21,64}$`.
pub(crate) fn is_modelled_probe_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("probe-") else {
        return false;
    };
    (21..=64).contains(&rest.chars().count())
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A deterministic name of `length` characters drawn from [`NAME_ALPHABET`], so
/// the length bound is exercised across the whole admitted alphabet rather than
/// on one hand-picked spelling.
pub(crate) fn name_of_length(length: usize) -> String {
    (0..length)
        .map(|i| char::from(NAME_ALPHABET[(i * 7 + length) % NAME_ALPHABET.len()]))
        .collect()
}

/// A monitor round-trips: create, get, list, delete, then every read of the
/// deleted name is `ResourceNotFoundException`.
///
/// The table spans the whole class the rule covers — the name's length and
/// alphabet, whether an `aggregationPeriod` was asked for and which, and how
/// many tags the monitor carries. The recorded case (`demo-monitor`,
/// `aggregationPeriod` 30, one tag) is one row.
#[tokio::test]
async fn a_monitor_round_trips_through_create_get_list_and_delete() {
    // (case, monitorName, request body, expected aggregationPeriod, expected tags)
    let cases: Vec<(&str, String, String, i64, Value)> = vec![
        (
            "the recorded case: one tag, period 30",
            "demo-monitor".to_string(),
            r#"{"monitorName":"demo-monitor","aggregationPeriod":30,"tags":{"team":"net"}}"#
                .to_string(),
            30,
            serde_json::json!({"team": "net"}),
        ),
        (
            "no period named: the documented default",
            "default-period".to_string(),
            r#"{"monitorName":"default-period"}"#.to_string(),
            60,
            serde_json::json!({}),
        ),
        (
            "period 60 named explicitly, three tags",
            "three_tags".to_string(),
            r#"{"monitorName":"three_tags","aggregationPeriod":60,"tags":{"a":"1","b":"2","c":"3"}}"#
                .to_string(),
            60,
            serde_json::json!({"a": "1", "b": "2", "c": "3"}),
        ),
        (
            "shortest name the pattern admits",
            name_of_length(1),
            format!(
                r#"{{"monitorName":"{}","aggregationPeriod":30}}"#,
                name_of_length(1)
            ),
            30,
            serde_json::json!({}),
        ),
        (
            "longest name the pattern admits",
            name_of_length(200),
            format!(r#"{{"monitorName":"{}"}}"#, name_of_length(200)),
            60,
            serde_json::json!({}),
        ),
    ];

    let router = router();
    for (case, name, body, expected_period, expected_tags) in cases {
        let expected_arn = format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:monitor/{name}");

        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "monitorArn"),
            expected_arn,
            "case {case:?}: expected monitorArn {expected_arn:?}, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "monitorName"),
            name,
            "case {case:?}: expected monitorName {name:?}, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "state"),
            "INACTIVE",
            "case {case:?}: a monitor with no probes is INACTIVE, got {created:?}"
        );
        assert_eq!(
            created.get("aggregationPeriod").and_then(Value::as_i64),
            Some(expected_period),
            "case {case:?}: expected aggregationPeriod {expected_period}, got {created:?}"
        );
        assert_eq!(
            created.get("tags"),
            Some(&expected_tags),
            "case {case:?}: expected tags {expected_tags:?}, got {created:?}"
        );

        // Read back through GetMonitor: every value comes from the stored
        // record, so it must agree with what the create reported.
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            string_at(&got, "monitorArn"),
            expected_arn,
            "case {case:?}: expected monitorArn {expected_arn:?}, got {got:?}"
        );
        assert_eq!(
            got.get("aggregationPeriod").and_then(Value::as_i64),
            Some(expected_period),
            "case {case:?}: expected aggregationPeriod {expected_period}, got {got:?}"
        );
        assert_eq!(
            got.get("tags"),
            Some(&expected_tags),
            "case {case:?}: expected tags {expected_tags:?}, got {got:?}"
        );
        assert_eq!(
            string_at(&got, "state"),
            "INACTIVE",
            "case {case:?}: a monitor with no probes stays INACTIVE, got {got:?}"
        );
        assert!(
            probes_of(&got).is_empty(),
            "case {case:?}: a monitor created without probes lists none, got {got:?}"
        );
        for member in ["createdAt", "modifiedAt"] {
            assert!(
                got.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: GetMonitor must report {member}, got {got:?}"
            );
        }

        let (status, listed) = list_monitors(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListMonitors 200, got {status} {listed:?}"
        );
        assert!(
            page_names(&listed).contains(&name),
            "case {case:?}: ListMonitors must include {name:?}, got {listed:?}"
        );

        let (status, deleted) = call(&router, "DELETE", &format!("/monitors/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteMonitor 200, got {status} {deleted:?}"
        );

        let (status, missing) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetMonitor 404 after the delete, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {missing:?}"
        );
        let (_, listed) = list_monitors(&router, "").await;
        assert!(
            !page_names(&listed).contains(&name),
            "case {case:?}: ListMonitors must not include the deleted {name:?}, got {listed:?}"
        );
    }
}

/// `aggregationPeriod` is one of the two values the model documents for it
/// ("Valid values are either `30` or `60`"), and `60` when the request names
/// none. The table carries both published values, the absent case, and values
/// either side of them — including `29`, one below the shape's own
/// `@range(min: 30)`.
///
/// The two refusal CLASSES are distinguished too: a value outside the shape's
/// own `@range` is a body the front door refuses (`BadRequestException`), while
/// one inside the range but outside the documented value list is refused by the
/// handler with the modelled `ValidationException`.
#[tokio::test]
async fn aggregation_period_accepts_only_the_documented_values() {
    // (case, body fragment, expected status, expected stored period, expected error code)
    let cases: &[(&str, &str, u16, Option<i64>, &str)] = &[
        ("the documented floor", r#","aggregationPeriod":30"#, 200, Some(30), ""),
        ("the documented default, named", r#","aggregationPeriod":60"#, 200, Some(60), ""),
        ("named nowhere: the documented default", "", 200, Some(60), ""),
        (
            "below the modelled range",
            r#","aggregationPeriod":29"#,
            400,
            None,
            "BadRequestException",
        ),
        (
            "between the two documented values",
            r#","aggregationPeriod":45"#,
            400,
            None,
            "ValidationException",
        ),
        (
            "above both documented values",
            r#","aggregationPeriod":90"#,
            400,
            None,
            "ValidationException",
        ),
        (
            "far above",
            r#","aggregationPeriod":3600"#,
            400,
            None,
            "ValidationException",
        ),
    ];

    let router = router();
    for (index, (case, fragment, expected_status, expected_period, expected_code)) in
        cases.iter().enumerate()
    {
        let name = format!("period-case-{index}");
        let body = format!(r#"{{"monitorName":"{name}"{fragment}}}"#);
        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {created:?}"
        );
        match expected_period {
            Some(period) => {
                assert_eq!(
                    created.get("aggregationPeriod").and_then(Value::as_i64),
                    Some(*period),
                    "case {case:?}: expected aggregationPeriod {period}, got {created:?}"
                );
                // What was STORED is what a later read reports.
                let (_, got) = get_monitor(&router, &name).await;
                assert_eq!(
                    got.get("aggregationPeriod").and_then(Value::as_i64),
                    Some(*period),
                    "case {case:?}: GetMonitor must report the stored period, got {got:?}"
                );
            }
            None => {
                assert_eq!(
                    error_code(&created),
                    *expected_code,
                    "case {case:?}: expected {expected_code}, got {created:?}"
                );
                let (status, missing) = get_monitor(&router, &name).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: a refused create stores nothing, got {status} {missing:?}"
                );
            }
        }
    }
}

/// A probe is stored under a minted `probeId`, reported from the record, and
/// listed by `GetMonitor`; `addressFamily` is derived from `destination`, and
/// the documented `destinationPort` and modelled `packetSize` bounds are
/// enforced.
///
/// The table spans protocol, address family, and each published bound of
/// `destinationPort` (`1`..`65536`) and `packetSize` (`56`..`8500`) plus one
/// value past it. The two refusal CLASSES are distinguished: a value past a
/// bound the MODEL declares is a body the front door refuses
/// (`BadRequestException`), while one that only breaks a rule the model states
/// in prose reaches the handler, which refuses it with the modelled
/// `ValidationException`.
#[tokio::test]
async fn a_probe_is_stored_and_its_address_family_follows_the_destination() {
    // (case, probe JSON, expected status, expected addressFamily, expected error code)
    let cases: &[(&str, &str, u16, &str, &str)] = &[
        (
            "the recorded case: ICMP to an IPv4 address",
            r#"{"sourceArn":"SOURCE","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}"#,
            200,
            "IPV4",
            "",
        ),
        (
            "ICMP to an IPv6 address",
            r#"{"sourceArn":"SOURCE","destination":"2001:db8::1","protocol":"ICMP","packetSize":1500}"#,
            200,
            "IPV6",
            "",
        ),
        (
            "TCP at the documented destinationPort floor",
            r#"{"sourceArn":"SOURCE","destination":"192.0.2.10","protocol":"TCP","destinationPort":1,"packetSize":56}"#,
            200,
            "IPV4",
            "",
        ),
        (
            "TCP at the modelled destinationPort ceiling",
            r#"{"sourceArn":"SOURCE","destination":"198.51.100.7","protocol":"TCP","destinationPort":65536,"packetSize":8500}"#,
            200,
            "IPV4",
            "",
        ),
        (
            "TCP to an IPv6 address at the packetSize floor",
            r#"{"sourceArn":"SOURCE","destination":"2001:db8::dead:beef","protocol":"TCP","destinationPort":443,"packetSize":56}"#,
            200,
            "IPV6",
            "",
        ),
        (
            "TCP with no destinationPort: required for TCP",
            r#"{"sourceArn":"SOURCE","destination":"203.0.113.1","protocol":"TCP","packetSize":56}"#,
            400,
            "",
            "ValidationException",
        ),
        (
            "destinationPort 0: one below the documented floor",
            r#"{"sourceArn":"SOURCE","destination":"203.0.113.2","protocol":"TCP","destinationPort":0,"packetSize":56}"#,
            400,
            "",
            "ValidationException",
        ),
        (
            "destinationPort 65537: one past the modelled ceiling",
            r#"{"sourceArn":"SOURCE","destination":"203.0.113.3","protocol":"TCP","destinationPort":65537,"packetSize":56}"#,
            400,
            "",
            "BadRequestException",
        ),
        (
            "packetSize 55: one below the modelled floor",
            r#"{"sourceArn":"SOURCE","destination":"203.0.113.4","protocol":"ICMP","packetSize":55}"#,
            400,
            "",
            "BadRequestException",
        ),
        (
            "packetSize 8501: one past the modelled ceiling",
            r#"{"sourceArn":"SOURCE","destination":"203.0.113.5","protocol":"ICMP","packetSize":8501}"#,
            400,
            "",
            "BadRequestException",
        ),
        (
            "a destination that is not an IP address",
            r#"{"sourceArn":"SOURCE","destination":"example.internal","protocol":"ICMP","packetSize":56}"#,
            400,
            "",
            "ValidationException",
        ),
    ];

    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"probe-host"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, probe, expected_status, expected_family, expected_code) in cases {
        let probe = probe.replace("SOURCE", &source_arn());
        let body = format!(r#"{{"probe":{probe}}}"#);
        let (status, created) = create_probe(&router, "probe-host", &body).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {created:?}"
        );
        if *expected_status != 200 {
            assert_eq!(
                error_code(&created),
                *expected_code,
                "case {case:?}: expected {expected_code}, got {created:?}"
            );
            continue;
        }

        let probe_id = string_at(&created, "probeId");
        assert!(
            is_modelled_probe_id(&probe_id),
            "case {case:?}: probeId must match ^probe-[a-z0-9A-Z-]{{21,64}}$, got {probe_id:?}"
        );
        assert!(
            minted.insert(probe_id.clone()),
            "case {case:?}: probeId {probe_id:?} was already minted for another probe"
        );
        let expected_probe_arn = format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:probe/{probe_id}");
        assert_eq!(
            string_at(&created, "probeArn"),
            expected_probe_arn,
            "case {case:?}: expected probeArn {expected_probe_arn:?}, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "addressFamily"),
            *expected_family,
            "case {case:?}: expected addressFamily {expected_family:?}, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "state"),
            "PENDING",
            "case {case:?}: a freshly created probe is PENDING, got {created:?}"
        );

        // The probe GetMonitor lists IS the stored record the create reported.
        let (status, got) = get_monitor(&router, "probe-host").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        let listed = probes_of(&got)
            .into_iter()
            .find(|listed| string_at(listed, "probeId") == probe_id);
        let Some(listed) = listed else {
            panic!("case {case:?}: GetMonitor must list probe {probe_id:?}, got {got:?}");
        };
        for member in [
            "probeArn",
            "sourceArn",
            "destination",
            "destinationPort",
            "protocol",
            "packetSize",
            "addressFamily",
            "state",
        ] {
            assert_eq!(
                listed.get(member),
                created.get(member),
                "case {case:?}: GetMonitor's {member} must equal what CreateProbe stored, \
                 got {listed:?} vs {created:?}"
            );
        }
    }

    assert_eq!(
        minted.len(),
        5,
        "expected the five admissible rows to be stored, got {minted:?}"
    );
}

/// A probe's tags are the union of the request-level `tags` and the ones nested
/// in `probe`, with the nested — more specific — pair winning a key both name.
#[tokio::test]
async fn probe_tags_come_from_both_places_the_request_can_name_them() {
    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"tagged"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    let body = format!(
        r#"{{"tags":{{"team":"net","owner":"request"}},
             "probe":{{"sourceArn":"{}","destination":"10.0.0.9","protocol":"ICMP",
                       "packetSize":56,"tags":{{"owner":"probe"}}}}}}"#,
        source_arn()
    );
    let (status, created) = create_probe(&router, "tagged", &body).await;
    assert_eq!(
        status, 200,
        "expected CreateProbe 200, got {status} {created:?}"
    );
    assert_eq!(
        created.get("tags"),
        Some(&serde_json::json!({"team": "net", "owner": "probe"})),
        "expected the merged tags with the nested pair winning, got {created:?}"
    );
}

/// A second create of a stored name is `ConflictException`; a repeat carrying
/// the same `clientToken` is a retry that replays the first answer.
#[tokio::test]
async fn a_repeat_create_conflicts_unless_it_replays_a_client_token() {
    let router = router();

    let (status, _) = create_monitor(&router, r#"{"monitorName":"once"}"#).await;
    assert_eq!(status, 200, "setup: the first CreateMonitor should succeed");
    let (status, conflict) = create_monitor(&router, r#"{"monitorName":"once"}"#).await;
    assert_eq!(
        status, 409,
        "case \"no client token\": expected 409, got {status} {conflict:?}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "case \"no client token\": expected ConflictException, got {conflict:?}"
    );

    let body = r#"{"monitorName":"retried","clientToken":"token-1","aggregationPeriod":30}"#;
    let (status, first) = create_monitor(&router, body).await;
    assert_eq!(status, 200, "expected the first create 200, got {first:?}");
    let (status, replayed) = create_monitor(&router, body).await;
    assert_eq!(
        status, 200,
        "case \"same client token\": a retry succeeds, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, first,
        "case \"same client token\": the retry must replay the first answer, got {replayed:?}"
    );

    // And a probe retry mints no second probe.
    let probe_body = format!(
        r#"{{"clientToken":"probe-token","probe":{{"sourceArn":"{}","destination":"10.0.0.1",
             "protocol":"ICMP","packetSize":56}}}}"#,
        source_arn()
    );
    let (status, first) = create_probe(&router, "retried", &probe_body).await;
    assert_eq!(status, 200, "expected the first CreateProbe 200, got {first:?}");
    let (status, replayed) = create_probe(&router, "retried", &probe_body).await;
    assert_eq!(
        status, 200,
        "case \"same probe client token\": a retry succeeds, got {status} {replayed:?}"
    );
    assert_eq!(
        string_at(&replayed, "probeId"),
        string_at(&first, "probeId"),
        "case \"same probe client token\": the retry must replay the first probe, got {replayed:?}"
    );
    let (_, got) = get_monitor(&router, "retried").await;
    assert_eq!(
        probes_of(&got).len(),
        1,
        "case \"same probe client token\": exactly one probe must be stored, got {got:?}"
    );
}

/// `ListMonitors` pages in `monitorName` order: each page holds at most
/// `maxResults` monitors, the cursor resumes exactly where the page ended, and
/// the final page carries no `nextToken`.
///
/// The table spans page sizes from one to past the result count, including the
/// model's own `maxResults` maximum.
#[tokio::test]
async fn list_monitors_pages_the_stored_monitors() {
    let router = router();
    let all: Vec<String> = (1..=5).map(|i| format!("monitor-{i}")).collect();
    for name in &all {
        let (status, created) = create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#))
            .await;
        assert_eq!(status, 200, "setup: CreateMonitor {name} -> {created:?}");
    }

    for page_size in [1_usize, 2, 3, 5, 25] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let (status, body) = list_monitors(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListMonitors 200, got {status} {body:?}"
            );
            let names = page_names(&body);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} monitors, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging must terminate, saw {seen:?}"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging must yield every monitor once, in name order, got {seen:?}"
        );
    }

    // A cursor this service never vended names no page of the result set.
    let (status, refused) = list_monitors(&router, "?nextToken=6e6f742d612d637572736f72").await;
    assert_eq!(
        status, 400,
        "case \"an unknown cursor\": expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"an unknown cursor\": expected ValidationException, got {refused:?}"
    );
}

/// `ListMonitors` filters on the `state` query parameter, and refuses a value
/// outside the `MonitorState` enum — which the member's plain `string` target
/// cannot express.
///
/// The catalog of states comes from the model's `MonitorState` enum
/// (`ACTIVE | DELETING | ERROR | INACTIVE | PENDING`).
#[tokio::test]
async fn list_monitors_filters_on_state() {
    const MONITOR_STATES: &[&str] = &["ACTIVE", "DELETING", "ERROR", "INACTIVE", "PENDING"];

    let router = router();
    for name in ["filter-a", "filter-b"] {
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
        assert_eq!(status, 200, "setup: CreateMonitor {name} -> {created:?}");
    }

    for state in MONITOR_STATES {
        let (status, body) = list_monitors(&router, &format!("?state={state}")).await;
        assert_eq!(
            status, 200,
            "case {state:?}: expected ListMonitors 200, got {status} {body:?}"
        );
        let expected: Vec<String> = if *state == "INACTIVE" {
            vec!["filter-a".to_string(), "filter-b".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            page_names(&body),
            expected,
            "case {state:?}: expected {expected:?}, got {body:?}"
        );
    }

    for bogus in ["pending", "BOGUS", ""] {
        let (status, refused) = list_monitors(&router, &format!("?state={bogus}")).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: a state outside MonitorState is refused, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {bogus:?}: expected ValidationException, got {refused:?}"
        );
    }
}

/// The `PENDING` -> `ACTIVE` transition is owed to the CLOCK: a monitor that has
/// a probe is `PENDING` straight after the call and `ACTIVE` once its due
/// instant has passed, while a monitor with no probes has nothing to aggregate,
/// so it is `INACTIVE` with nothing due however long anyone waits.
#[tokio::test]
async fn a_monitor_with_probes_activates_when_the_clock_makes_it_due() {
    let router = router();
    let with_probe = format!(
        r#"{{"monitorName":"activating","probes":[{{"sourceArn":"{}","destination":"10.0.0.1",
             "protocol":"ICMP","packetSize":56,"probeTags":{{"role":"edge"}}}}]}}"#,
        source_arn()
    );
    let (status, created) = create_monitor(&router, &with_probe).await;
    assert_eq!(
        status, 200,
        "expected CreateMonitor 200, got {status} {created:?}"
    );
    assert_eq!(
        string_at(&created, "state"),
        "PENDING",
        "case \"before the activation span\": expected PENDING, got {created:?}"
    );

    let (status, idle) = create_monitor(&router, r#"{"monitorName":"idle"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor idle -> {idle:?}");

    // Straight after the call, both the monitor and its probe are transient.
    let (_, got) = get_monitor(&router, "activating").await;
    assert_eq!(
        string_at(&got, "state"),
        "PENDING",
        "case \"before the activation span\": expected PENDING, got {got:?}"
    );
    let probe = probes_of(&got).first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        string_at(&probe, "state"),
        "PENDING",
        "case \"before the activation span\": the probe is PENDING too, got {got:?}"
    );
    assert_eq!(
        probe.get("tags"),
        Some(&serde_json::json!({"role": "edge"})),
        "a probe created with the monitor keeps its probeTags, got {probe:?}"
    );
    let probe_id = string_at(&probe, "probeId");
    assert!(
        is_modelled_probe_id(&probe_id),
        "a probe created with the monitor gets a modelled probeId, got {probe_id:?}"
    );

    std::thread::sleep(PAST_ACTIVATION);

    let (_, settled) = get_monitor(&router, "activating").await;
    assert_eq!(
        string_at(&settled, "state"),
        "ACTIVE",
        "case \"after the activation span\": expected ACTIVE, got {settled:?}"
    );
    let probe = probes_of(&settled).first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        string_at(&probe, "state"),
        "ACTIVE",
        "case \"after the activation span\": the probe is ACTIVE too, got {settled:?}"
    );
    // The reported state is what the filter selects on.
    let (_, listed) = list_monitors(&router, "?state=ACTIVE").await;
    assert_eq!(
        page_names(&listed),
        vec!["activating".to_string()],
        "case \"after the activation span\": the ACTIVE filter selects it, got {listed:?}"
    );

    let (_, idle) = get_monitor(&router, "idle").await;
    assert_eq!(
        string_at(&idle, "state"),
        "INACTIVE",
        "case \"a monitor with no probes\": nothing is due, got {idle:?}"
    );

    // Deleting the last probe leaves the monitor with nothing to aggregate.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/monitors/activating/probes/{probe_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteProbe 200, got {status} {deleted:?}"
    );
    let (_, emptied) = get_monitor(&router, "activating").await;
    assert!(
        probes_of(&emptied).is_empty(),
        "the deleted probe must be gone from GetMonitor, got {emptied:?}"
    );
    assert_eq!(
        string_at(&emptied, "state"),
        "INACTIVE",
        "a monitor whose last probe was deleted has nothing to aggregate, got {emptied:?}"
    );
}

/// Every operation that names a resource answers `ResourceNotFoundException`
/// when the resource is not stored — the monitor for all of them, and the probe
/// for `DeleteProbe`.
#[tokio::test]
async fn an_unstored_resource_is_reported_as_not_found() {
    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"present"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");
    let absent_probe = format!("probe-{}", "a".repeat(21));

    // (case, method, path, body)
    let cases: Vec<(&str, &str, String, Option<String>)> = vec![
        ("GetMonitor on an unknown monitor", "GET", "/monitors/absent".to_string(), None),
        (
            "CreateProbe on an unknown monitor",
            "POST",
            "/monitors/absent/probes".to_string(),
            Some(format!(
                r#"{{"probe":{{"sourceArn":"{}","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}}}}"#,
                source_arn()
            )),
        ),
        (
            "DeleteMonitor on an unknown monitor",
            "DELETE",
            "/monitors/absent".to_string(),
            None,
        ),
        (
            "DeleteProbe on an unknown monitor",
            "DELETE",
            format!("/monitors/absent/probes/{absent_probe}"),
            None,
        ),
        (
            "DeleteProbe on an unknown probe of a stored monitor",
            "DELETE",
            format!("/monitors/present/probes/{absent_probe}"),
            None,
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, refused) = call(&router, method, &path, body.as_deref()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
    }
}

/// The operations this crate implements answer from state, and every operation
/// this service models is implemented now: there is no scaffolded row left to
/// assert a 501 for.
///
/// `UpdateProbe` was the last one — `tests/all/update_probe.rs` asserts what it
/// does — and `UpdateMonitor`, `TagResource`, `UntagResource`,
/// `ListTagsForResource` and `GetProbe` are pinned by
/// `tests/all/update_monitor.rs`, `tests/all/tagging.rs`,
/// `tests/all/resource_tags.rs` and
/// `tests/all/monitor_lifecycle_and_front_door.rs`. The 501 census itself lives
/// in the handlers: an operation nobody has written yet carries the
/// `//! SCAFFOLDED, NOT IMPLEMENTED:` header, and this crate has none.
#[tokio::test]
async fn every_modelled_operation_answers_from_state() {
    let router = router();
    let (status, _) = create_monitor(&router, r#"{"monitorName":"no-longer-stubbed"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor should succeed");

    // The route that used to answer 501 now answers the modelled 404 for a
    // probe id nothing minted, which is what an implemented handler does.
    let path = format!(
        "/monitors/no-longer-stubbed/probes/probe-{}",
        "a".repeat(21)
    );
    let (status, refused) = call(&router, "PATCH", &path, Some(r#"{"packetSize":56}"#)).await;
    assert_eq!(
        status, 404,
        "UpdateProbe is implemented, so an unknown probe is 404, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {refused:?}"
    );
}
