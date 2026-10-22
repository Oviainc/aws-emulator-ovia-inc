// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `TagResource` and `UntagResource`.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rules under test:
//!
//! * a tagging call addresses its target by the ARN this service MINTED, which
//!   resolves to the stored monitor or the stored probe that carries it, and to
//!   nothing otherwise — an ARN no stored resource carries is HTTP 404
//!   `ResourceNotFoundException`;
//! * `TagResource` adds exactly the pairs it names to that one resource's stored
//!   tag map, overwriting the value of a key the resource already carries and
//!   leaving every other key, and every other resource's tags, alone;
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
//! `ListTagsForResource` is still a scaffold with an issue of its own, and so is
//! `GetProbe`, so the read-back here goes through `GetMonitor` (implemented),
//! whose `probes` list IS the stored probe map. `tests/all/tagging.rs` reuses
//! this suite's harness for the read paths beyond `GetMonitor`.

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

/// One row of the `TagResource` table: the same fixture, the resource the call
/// addresses, the pairs it writes, and the tags each resource is left with.
struct TagCase {
    case: &'static str,
    monitor_tags: Value,
    probe_tags: Value,
    target: Target,
    written: Value,
    want_monitor_tags: Value,
    want_probe_tags: Value,
}

/// A JSON tag map built from `(key, value)` pairs — for the rows whose keys or
/// values are computed rather than written out.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
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

pub(crate) async fn get_monitor(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
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

/// `POST /tags/{resourceArn}` with `tags` in the body — the pairs to add.
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &Value,
) -> (u16, Value) {
    let path = format!("/tags/{}", encode_label(resource_arn));
    call(router, "POST", &path, Some(&format!(r#"{{"tags":{tags}}}"#))).await
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
pub(crate) fn only_probe(body: &Value) -> Value {
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
pub(crate) async fn fixture(
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

/// `TagResource` adds exactly the named pairs to the resource the ARN names,
/// overwriting a key it already carries and touching nothing else — read back
/// through `GetMonitor`.
///
/// The table spans the class the rule covers: which KIND of resource the ARN
/// names (monitor or probe), how many pairs the call writes (none, one,
/// several), whether a written key is new or already stored, and the key and
/// value lengths the modelled `TagMap` admits at its bounds (key
/// `@length(1..128)`, value `@length(0..256)` —
/// smithy_query(query="shape",shape="TagMap")). The recorded case (one new key
/// on a monitor that already carries one) is one row.
#[tokio::test]
async fn tag_resource_adds_exactly_the_named_pairs() {
    // Owned, because the bound rows build their maps from them.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    let cases: Vec<TagCase> = vec![
        TagCase {
            case: "the recorded case: one new key beside the one a monitor carries",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            written: serde_json::json!({"env": "prod"}),
            want_monitor_tags: serde_json::json!({"team": "net", "env": "prod"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        TagCase {
            case: "a key the monitor already carries takes the new value",
            monitor_tags: serde_json::json!({"env": "dev", "team": "net"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            written: serde_json::json!({"env": "prod"}),
            want_monitor_tags: serde_json::json!({"env": "prod", "team": "net"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        TagCase {
            case: "three keys at once onto an untagged monitor",
            monitor_tags: serde_json::json!({}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            written: serde_json::json!({"a": "1", "b": "2", "c": "3"}),
            want_monitor_tags: serde_json::json!({"a": "1", "b": "2", "c": "3"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        TagCase {
            case: "an empty tag map leaves the stored tags as they were",
            monitor_tags: serde_json::json!({"keep": "me"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            written: serde_json::json!({}),
            want_monitor_tags: serde_json::json!({"keep": "me"}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        TagCase {
            case: "a value at the TagValue minimum length, 0",
            monitor_tags: serde_json::json!({"other": "kept"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            written: serde_json::json!({"blank": ""}),
            want_monitor_tags: serde_json::json!({"other": "kept", "blank": ""}),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        TagCase {
            case: "a key at the TagKey maximum, 128, and a value at the TagValue maximum, 256",
            monitor_tags: serde_json::json!({}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Monitor,
            written: tag_map(&[(longest_key.as_str(), longest_value.as_str())]),
            want_monitor_tags: tag_map(&[(longest_key.as_str(), longest_value.as_str())]),
            want_probe_tags: serde_json::json!({"role": "edge"}),
        },
        TagCase {
            case: "a probe, by its probe ARN: a new key beside the one it carries",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"role": "edge"}),
            target: Target::Probe,
            written: serde_json::json!({"owner": "probe"}),
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({"role": "edge", "owner": "probe"}),
        },
        TagCase {
            case: "a probe key that is also a MONITOR key: only the probe changes",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({"team": "probe-team"}),
            target: Target::Probe,
            written: serde_json::json!({"team": "written"}),
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({"team": "written"}),
        },
        TagCase {
            case: "several keys at once onto an untagged probe",
            monitor_tags: serde_json::json!({"team": "net"}),
            probe_tags: serde_json::json!({}),
            target: Target::Probe,
            written: serde_json::json!({"a": "1", "b": "2"}),
            want_monitor_tags: serde_json::json!({"team": "net"}),
            want_probe_tags: serde_json::json!({"a": "1", "b": "2"}),
        },
    ];

    let router = router();
    for (index, row) in cases.into_iter().enumerate() {
        let TagCase {
            case,
            monitor_tags,
            probe_tags,
            target,
            written,
            want_monitor_tags: want_monitor,
            want_probe_tags: want_probe,
        } = row;
        let name = format!("tag-{index}");
        let (monitor_arn, probe_arn) = fixture(&router, &name, &monitor_tags, &probe_tags).await;
        let resource_arn = match target {
            Target::Monitor => monitor_arn,
            Target::Probe => probe_arn,
        };

        let (status, body) = tag_resource(&router, &resource_arn, &written).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource 200, got {status} {body:?}"
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

/// Tag writes of disjoint keys converge on the union of all of them.
///
/// Four `TagResource` calls are in flight at once against the same monitor, so
/// the order they land in is not fixed; whatever it is, no write may drop
/// another's key or the keys the monitor was created with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tag_writes_converge_on_the_union_of_their_keys() {
    let router = router();
    let (monitor_arn, _) = fixture(
        &router,
        "tag-converging",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;

    let first = serde_json::json!({"a": "1"});
    let second = serde_json::json!({"b": "2"});
    // One concurrent write names a key another write also names, with the same
    // value, so convergence does not depend on which of the two landed last.
    let third = serde_json::json!({"c": "3", "b": "2"});
    let fourth = serde_json::json!({"d": "4"});

    let answers = tokio::join!(
        tag_resource(&router, &monitor_arn, &first),
        tag_resource(&router, &monitor_arn, &second),
        tag_resource(&router, &monitor_arn, &third),
        tag_resource(&router, &monitor_arn, &fourth),
    );
    for (index, (status, body)) in [answers.0, answers.1, answers.2, answers.3]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            status, 200,
            "case \"concurrent write {index}\": expected TagResource 200, got {status} {body:?}"
        );
    }

    let want = serde_json::json!({"team": "net", "a": "1", "b": "2", "c": "3", "d": "4"});
    let (status, got) = get_monitor(&router, "tag-converging").await;
    assert_eq!(status, 200, "expected GetMonitor 200, got {status} {got:?}");
    assert_eq!(
        got.get("tags"),
        Some(&want),
        "concurrent tag writes must converge on {want}, got {got:?}"
    );
}

/// The merged set is what this emulator's per-resource tag ceiling applies to: a
/// write that would push a resource past it is refused, while one that only
/// overwrites keys it already carries is not.
///
/// The ceiling's value is the published maximum of `TagMap`
/// (`@length(min: 0, max: 200)`, smithy_query(query="shape",shape="TagMap")),
/// the most pairs a response can carry; this exercises exactly 200, then one
/// past it.
#[tokio::test]
async fn a_tag_write_past_the_tag_map_maximum_is_refused_and_persists_nothing() {
    let router = router();
    let keys: Vec<String> = (0..200).map(|index| format!("key-{index:03}")).collect();
    let full = tag_map(
        &keys
            .iter()
            .map(|key| (key.as_str(), "v"))
            .collect::<Vec<_>>(),
    );
    let (monitor_arn, _) = fixture(
        &router,
        "tag-at-the-bound",
        &full,
        &serde_json::json!({"role": "edge"}),
    )
    .await;

    // At the bound: overwriting a key the monitor already carries keeps it at 200.
    let overwrite = tag_map(&[(keys[0].as_str(), "overwritten")]);
    let (status, body) = tag_resource(&router, &monitor_arn, &overwrite).await;
    assert_eq!(
        status, 200,
        "case \"200 stored, overwriting one of them\": expected 200, got {status} {body:?}"
    );

    // One past it: a 201st key cannot be stored.
    let (status, refused) =
        tag_resource(&router, &monitor_arn, &serde_json::json!({"key-200": "v"})).await;
    assert_eq!(
        status, 400,
        "case \"200 stored, one new key\": expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case \"200 stored, one new key\": expected ValidationException, got {refused:?}"
    );

    let mut want = full.clone();
    if let Value::Object(map) = &mut want {
        map.insert(keys[0].clone(), Value::String("overwritten".to_string()));
    }
    let (_, got) = get_monitor(&router, "tag-at-the-bound").await;
    assert_eq!(
        got.get("tags"),
        Some(&want),
        "the refused write must persist nothing, got {got:?}"
    );
}

/// An ARN no stored resource carries is refused by BOTH tagging writes with HTTP
/// 404 `ResourceNotFoundException`, whatever shape it has.
///
/// The rows differ along WHY nothing carries the ARN: the name was never
/// created, the resource was deleted, the probe id is not one that was minted,
/// the region or account is not the one the store minted into, and the ARN names
/// another service entirely.
#[tokio::test]
async fn tagging_an_arn_no_resource_carries_is_not_found() {
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

        // The same ARN resolution, so the same refusal, for the write side.
        let (status, body) =
            tag_resource(&router, &resource_arn, &serde_json::json!({"env": "prod"})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected TagResource 404, got {status} {body:?}"
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
        "a refused tagging call must not touch the monitor's tags, got {got:?}"
    );
    assert_eq!(
        string_at(&got, "monitorArn"),
        live_monitor_arn,
        "the stored monitor ARN is what the read reports, got {got:?}"
    );
    assert_eq!(
        only_probe(&got).get("tags"),
        Some(&serde_json::json!({"role": "edge"})),
        "a refused tagging call must not touch the probe's tags, got {got:?}"
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

/// The same rule for the write side: `modifiedAt` moves when the tag write
/// changed the stored map, and does not when every pair it carried was already
/// stored — such a call modified nothing.
#[tokio::test]
async fn modified_at_moves_only_when_a_tag_write_changed_the_stored_map() {
    let router = router();
    let (monitor_arn, _) = fixture(
        &router,
        "tag-modified",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;

    let timestamp = |body: &Value, member: &str| -> f64 {
        body.get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("GetMonitor must report {member}, got {body:?}"))
    };

    let (_, before) = get_monitor(&router, "tag-modified").await;
    let created_at = timestamp(&before, "createdAt");

    // (case, the pairs written, whether the monitor was modified by them)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "an empty tag map",
            serde_json::json!({}),
            false,
        ),
        (
            "the value the key already has",
            serde_json::json!({"team": "net"}),
            false,
        ),
        (
            "a new value for a stored key",
            serde_json::json!({"team": "ops"}),
            true,
        ),
        (
            "a key the monitor does not carry",
            serde_json::json!({"env": "prod"}),
            true,
        ),
    ];

    let mut modified_at = timestamp(&before, "modifiedAt");
    assert!(
        (modified_at - created_at).abs() < f64::EPSILON,
        "a freshly created monitor has modifiedAt == createdAt, got {before:?}"
    );
    for (case, written, modifies) in cases {
        let (status, body) = tag_resource(&router, &monitor_arn, &written).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource 200, got {status} {body:?}"
        );
        let (_, after) = get_monitor(&router, "tag-modified").await;
        let now_modified_at = timestamp(&after, "modifiedAt");
        if modifies {
            assert!(
                now_modified_at > modified_at,
                "case {case:?}: the write modified the monitor, so modifiedAt must move \
                 past {modified_at}, got {now_modified_at} in {after:?}"
            );
        } else {
            assert!(
                (now_modified_at - modified_at).abs() < f64::EPSILON,
                "case {case:?}: the write modified nothing, so modifiedAt must stay \
                 {modified_at}, got {now_modified_at} in {after:?}"
            );
        }
        assert!(
            (timestamp(&after, "createdAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: createdAt never moves, got {after:?}"
        );
        modified_at = now_modified_at;
    }
}
