// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `UntagResource`.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rule under test:
//!
//! * a tagging call addresses its target by the ARN this service MINTED, which
//!   resolves to the stored monitor or the stored probe that carries it, and to
//!   nothing otherwise — an ARN no stored resource carries is HTTP 404
//!   `ResourceNotFoundException`;
//! * `UntagResource` removes exactly the keys it names from that one resource's
//!   stored tag map, leaving every other key and every other resource's tags
//!   alone, and removing a key the resource does not carry is a no-op success
//!   (the call is idempotent);
//! * what is left is what every later read of the resource reports — here
//!   `GetMonitor`'s `tags` for a monitor and its `probes[].tags` for a probe,
//!   which is the same stored map the tagging call wrote;
//! * the resource's `modifiedAt` moves only when the call actually changed the
//!   stored tag map.
//!
//! `ListTagsForResource` and `TagResource` are still scaffolds with issues of
//! their own, so the read-back here goes through `GetMonitor` (implemented) and
//! the tags under test are assigned by `CreateMonitor`.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` naming this service's signing name
/// (`networkmonitor`) — how the router picks the service — and the region in its
/// credential scope, which is the region every ARN the service mints carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/networkmonitor/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The region in [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// Which stored resource a case's `resourceArn` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Monitor,
    Probe,
}

/// One row of the removal table: a monitor-plus-probe fixture, the resource the
/// call addresses, the keys it removes, and the tags each resource is left with.
struct Case {
    case: &'static str,
    monitor_tags: Value,
    probe_tags: Value,
    target: Target,
    removed: &'static [&'static str],
    want_monitor_tags: Value,
    want_probe_tags: Value,
}

fn router() -> MultiServiceRouter {
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
async fn call(
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
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `resourceArn` is bound to a NON-greedy `@httpLabel` (`/tags/{resourceArn}`),
/// so an ARN travels as ONE path segment with its `/` and `:` percent-encoded —
/// exactly what an SDK client sends.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

async fn create_monitor(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "POST", "/monitors", Some(body)).await
}

async fn get_monitor(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &format!("/monitors/{name}"), None).await
}

async fn delete_monitor(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/monitors/{name}"), None).await
}

/// `DELETE /tags/{resourceArn}?tagKeys=..&tagKeys=..` — the `tagKeys` list is
/// bound to `@httpQuery("tagKeys")`, so each key is its own query pair.
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[&str],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    let path = format!("/tags/{}?{query}", encode_label(resource_arn));
    call(router, "DELETE", &path, None).await
}

/// A subnet ARN to use as a probe's `sourceArn`; the model types it as `Arn`
/// with `@length(20..2048)` and `@pattern("^arn:.*$")`.
fn source_arn() -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0123456789abcdef0")
}

fn monitor_arn(name: &str) -> String {
    format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:monitor/{name}")
}

/// The single probe a `GetMonitor` body reports.
fn only_probe(body: &Value) -> Value {
    let probes = body
        .get("probes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        probes.len(),
        1,
        "the fixture monitor carries exactly one probe, got {body:?}"
    );
    probes[0].clone()
}

/// Create a monitor carrying `monitor_tags` and one probe carrying
/// `probe_tags`, and answer the monitor's and the probe's ARNs.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    monitor_tags: &Value,
    probe_tags: &Value,
) -> (String, String) {
    let body = format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":30,"tags":{monitor_tags},
             "probes":[{{"sourceArn":"{}","destination":"10.0.0.1","protocol":"ICMP",
                        "packetSize":56,"probeTags":{probe_tags}}}]}}"#,
        source_arn()
    );
    let (status, created) = create_monitor(router, &body).await;
    assert_eq!(
        status, 200,
        "fixture {name:?}: expected CreateMonitor 200, got {status} {created:?}"
    );
    let (status, got) = get_monitor(router, name).await;
    assert_eq!(
        status, 200,
        "fixture {name:?}: expected GetMonitor 200, got {status} {got:?}"
    );
    let probe = only_probe(&got);
    (
        string_at(&got, "monitorArn"),
        string_at(&probe, "probeArn"),
    )
}

/// `UntagResource` removes exactly the named keys from the resource the ARN
/// names, and nothing else — read back through `GetMonitor`.
///
/// The table spans the class the rule covers: which KIND of resource the ARN
/// names (monitor or probe), how many tags it starts with, how many keys the
/// call removes (one, several, all), and whether those keys are present at all.
/// The recorded case (remove the one tag a monitor carries) is one row.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    let cases: Vec<Case> = vec![
        Case {
            case: "the recorded case: the one tag a monitor carries",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["team"],
            want_monitor_tags: serde_json::json!({}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "one key out of three on a monitor",
            monitor_tags: serde_json::json!({"a": "1", "b": "2", "c": "3"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["b"],
            want_monitor_tags: serde_json::json!({"a": "1", "c": "3"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "several keys in one call",
            monitor_tags: serde_json::json!({"a": "1", "b": "2", "c": "3"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["a", "c"],
            want_monitor_tags: serde_json::json!({"b": "2"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "every key a monitor carries",
            monitor_tags: serde_json::json!({"a": "1", "b": "2"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["a", "b"],
            want_monitor_tags: serde_json::json!({}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "a key the monitor does not carry: removal is idempotent",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["absent"],
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "the same key twice in one call",
            monitor_tags: serde_json::json!({"team": "net", "keep": "me"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["team", "team"],
            want_monitor_tags: serde_json::json!({"keep": "me"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "a monitor with no tags at all",
            monitor_tags: serde_json::json!({}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            removed: &["team"],
            want_monitor_tags: serde_json::json!({}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        Case {
            case: "a probe's own tag, by its probe ARN",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge", "owner": "probe"}),
            target: Target::Probe,
            removed: &["role"],
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({"owner": "probe"}),
        },
        Case {
            case: "every tag a probe carries",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge", "owner": "probe"}),
            target: Target::Probe,
            removed: &["owner", "role"],
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({}),
        },
        Case {
            case: "a key the probe does not carry, and one the MONITOR carries",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Probe,
            removed: &["absent", "team"],
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
    ];

    let router = router();
    for (index, row) in cases.into_iter().enumerate() {
        let Case {
            case,
            monitor_tags,
            probe_tags,
            target,
            removed,
            want_monitor_tags: want_monitor,
            want_probe_tags: want_probe,
        } = row;
        let name = format!("untag-{index}");
        let (monitor_arn, probe_arn) = fixture(&router, &name, &monitor_tags, &probe_tags).await;
        let resource_arn = match target {
            Target::Monitor => monitor_arn,
            Target::Probe => probe_arn,
        };

        let (status, body) = untag_resource(&router, &resource_arn, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource 200, got {status} {body:?}"
        );

        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            got.get("tags"),
            Some(&want_monitor),
            "case {case:?}: expected monitor tags {want_monitor}, got {got:?}"
        );
        let probe = only_probe(&got);
        assert_eq!(
            probe.get("tags"),
            Some(&want_probe),
            "case {case:?}: expected probe tags {want_probe}, got {probe:?}"
        );
    }
}

/// An ARN no stored resource carries is HTTP 404 `ResourceNotFoundException`,
/// whatever shape it has.
///
/// The rows differ along WHY nothing carries the ARN: the name was never
/// created, the resource was deleted, the probe id is not one that was minted,
/// the region or account is not the one the store minted into, and the ARN names
/// another service entirely.
#[tokio::test]
async fn untagging_an_arn_no_resource_carries_is_not_found() {
    let router = router();
    let (live_monitor_arn, live_probe_arn) = fixture(
        &router,
        "untag-404",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;

    // A monitor that existed and was deleted: its ARN stops resolving.
    let (deleted_monitor_arn, _) = fixture(
        &router,
        "untag-deleted",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;
    let (status, body) = delete_monitor(&router, "untag-deleted").await;
    assert_eq!(
        status, 200,
        "expected DeleteMonitor 200, got {status} {body:?}"
    );

    let probe_id = live_probe_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let cases: Vec<(&str, String)> = vec![
        (
            "a monitor name that was never created",
            monitor_arn("never-created"),
        ),
        ("a monitor that was deleted", deleted_monitor_arn),
        (
            "a probe id that was never minted",
            format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:probe/probe-000000000000000000000000"),
        ),
        (
            "the monitor's ARN in another region",
            format!("arn:aws:networkmonitor:eu-west-1:{ACCOUNT}:monitor/untag-404"),
        ),
        (
            "the monitor's ARN under another account",
            format!("arn:aws:networkmonitor:{REGION}:111122223333:monitor/untag-404"),
        ),
        (
            "a probe ARN naming the monitor instead of the probe",
            format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:probe/untag-404"),
        ),
        (
            "a well-formed ARN of another service",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-{probe_id}"),
        ),
    ];

    for (case, resource_arn) in cases {
        let (status, body) = untag_resource(&router, &resource_arn, &["team"]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected UntagResource 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // The refusals changed nothing: the live resources keep their tags.
    let (status, got) = get_monitor(&router, "untag-404").await;
    assert_eq!(status, 200, "expected GetMonitor 200, got {status} {got:?}");
    assert_eq!(
        got.get("tags"),
        Some(&serde_json::json!({"team": "net"})),
        "a refused UntagResource must not touch the monitor's tags, got {got:?}"
    );
    assert_eq!(
        string_at(&got, "monitorArn"),
        live_monitor_arn,
        "the stored monitor ARN is what the read reports, got {got:?}"
    );
    assert_eq!(
        only_probe(&got).get("tags"),
        Some(&serde_json::json!({"role": "edge"})),
        "a refused UntagResource must not touch the probe's tags, got {got:?}"
    );
}

/// `modifiedAt` moves when the call removed a key, and does not when the call
/// removed nothing — a no-op modified no resource.
#[tokio::test]
async fn modified_at_moves_only_when_a_key_was_removed() {
    let router = router();
    let (monitor_arn, _) = fixture(
        &router,
        "untag-modified",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;

    let timestamp = |body: &Value, member: &str| -> f64 {
        body.get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("GetMonitor must report {member}, got {body:?}"))
    };

    let (_, before) = get_monitor(&router, "untag-modified").await;
    let created_at = timestamp(&before, "createdAt");
    assert!(
        (timestamp(&before, "modifiedAt") - created_at).abs() < f64::EPSILON,
        "a freshly created monitor has modifiedAt == createdAt, got {before:?}"
    );

    // Removing a key the monitor does not carry changes nothing.
    let (status, body) = untag_resource(&router, &monitor_arn, &["absent"]).await;
    assert_eq!(
        status, 200,
        "expected UntagResource 200 for an absent key, got {status} {body:?}"
    );
    let (_, after_noop) = get_monitor(&router, "untag-modified").await;
    let noop_modified_at = timestamp(&after_noop, "modifiedAt");
    assert!(
        (noop_modified_at - created_at).abs() < f64::EPSILON,
        "a no-op UntagResource modified nothing: expected modifiedAt {created_at}, \
         got {noop_modified_at} in {after_noop:?}"
    );

    // Removing a key the monitor carries is a modification.
    let (status, body) = untag_resource(&router, &monitor_arn, &["team"]).await;
    assert_eq!(
        status, 200,
        "expected UntagResource 200 for a present key, got {status} {body:?}"
    );
    let (_, after_removal) = get_monitor(&router, "untag-modified").await;
    let removed_modified_at = timestamp(&after_removal, "modifiedAt");
    assert!(
        removed_modified_at > created_at,
        "removing a tag modifies the monitor: expected modifiedAt past {created_at}, \
         got {removed_modified_at} in {after_removal:?}"
    );
    assert!(
        (timestamp(&after_removal, "createdAt") - created_at).abs() < f64::EPSILON,
        "createdAt never moves, got {after_removal:?}"
    );
}
