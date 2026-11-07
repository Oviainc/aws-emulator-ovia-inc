// SPDX-License-Identifier: CC0-1.0

//! Which success STATUS a tag mutation of this service answers with, driven over the
//! wire.
//!
//! The rule under test: a successful call that only writes a resource's tag set and
//! reports nothing back about it — the two mutations of `/tags/{resourceArn}`, whose
//! modelled outputs (`TagResourceOutput`, `UntagResourceOutput`) declare no members —
//! is answered with 204 No Content, because the response carries no entity for a caller
//! to parse. This service's model leaves both success codes defaulted (`@http` carries
//! no `code:`), so Smithy's default of 200 is not a statement about it, and the status
//! this suite asserts is written as the literal the recorded responses fix rather than
//! read from the service, so a handler that drifts back to 200 fails here.
//!
//! The tables span what the rule generalises over: which mutation, which KIND of
//! resource the ARN names (a monitor, a probe), how many pairs or keys the call carries
//! (none, one, several, the modelled `TagMap` maximum), the key and value form (ASCII,
//! non-ASCII, an empty value, each published length bound), how many calls the set was
//! built up over, and whether the call changes anything at all. The recorded case (one
//! pair onto a monitor, then one key removed again) is one row of each.
//!
//! Two boundaries are pinned beside it, so the rule is not read as "every success of
//! this service": a success that DOES report content keeps 200, and so do
//! `DeleteMonitor` and `DeleteProbe`, whose outputs also declare no members but which
//! the recorded AWS responses answer with the defaulted 200. A refusal keeps its own
//! status, and no 204 is an excuse for doing nothing: every row reads the tag set back
//! through `ListTagsForResource`.
//!
//! The harness, the published `TagKey`/`TagValue`/`TagMap` bounds, the tag-map builders
//! and the resource-kind dimension are the ones `resource_tags.rs` already owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{
    call, deactivate_probe, filler, fixture, get_monitor, list_tags, many_tags, only_probe, router,
    tag_map, tag_resource, untag_resource, Target, MAX_TAGS, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
    TAG_MUTATION_STATUS, TAG_NOT_FOUND_CODE,
};

/// The status a success that reports content carries — Smithy's protocol default, which
/// every operation outside the tag-mutation class leaves in place.
const OK: u16 = 200;

/// The keys of one `UntagResource` call, from borrowed literals.
fn keys(named: &[&str]) -> Vec<String> {
    named.iter().map(|key| (*key).to_string()).collect()
}

/// The keys of a tag object, in the order it holds them — so a removal naming "every
/// key" is built from the same source the write used.
fn keys_of(tags: &Value) -> Vec<String> {
    tags.as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// Assert that `status`/`body` is a success reporting no content: the status the
/// recording fixes, no error code, and nothing for a client to parse.
fn assert_no_content(case: &str, operation: &str, status: u16, body: &Value) {
    assert_eq!(
        status, TAG_MUTATION_STATUS,
        "case {case:?}: expected {operation} to answer {TAG_MUTATION_STATUS} No Content, \
         got {status} {body:?}"
    );
    assert!(
        body.get("__type").is_none(),
        "case {case:?}: expected {operation} to answer no error, got {body:?}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: expected {operation} to report no content, got {body:?}"
    );
}

/// Assert that `ListTagsForResource` — the content-bearing reader of the same stored map
/// — reports `expected` for `resource_arn`.
async fn assert_tags_readable(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
    expected: &Value,
) {
    let (status, listed) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, OK,
        "case {case:?}: expected ListTagsForResource {OK}, got {status} {listed:?}"
    );
    assert_eq!(
        listed.get("tags"),
        Some(expected),
        "case {case:?}: expected the tag set {expected} to be readable back, got {listed:?}"
    );
}

/// Create the row's fixture and answer the ARN of the resource it addresses.
async fn addressed_resource(
    router: &MultiServiceRouter,
    name: &str,
    target: Target,
    initial: &Value,
) -> String {
    let (monitor_arn, probe_arn) = match target {
        Target::Monitor => fixture(router, name, initial, &json!({"role": "edge"})).await,
        Target::Probe => fixture(router, name, &json!({"team": "net"}), initial).await,
    };
    match target {
        Target::Monitor => monitor_arn,
        Target::Probe => probe_arn,
    }
}

/// One row of the `TagResource` table.
struct TagRow {
    case: &'static str,
    target: Target,
    /// The tags the addressed resource starts with.
    initial: Value,
    /// The successive `TagResource` calls this row makes.
    calls: Vec<Value>,
    /// The tag set the addressed resource must carry once the calls are done.
    expected: Value,
}

/// Every successful `TagResource` answers 204 with no content, whatever the request
/// carried and whichever resource kind it addressed — and the tag set it left behind is
/// the one the content-bearing reader reports.
#[tokio::test]
async fn a_successful_tag_write_answers_no_content() {
    // Owned, because the length-bound row builds its map from them.
    let longest_key = filler(MAX_TAG_KEY_LEN, 3);
    let longest_value = filler(MAX_TAG_VALUE_LEN, 7);

    let rows: Vec<TagRow> = vec![
        TagRow {
            case: "the recorded case: one new pair on a monitor",
            target: Target::Monitor,
            initial: tag_map(&[("team", "net")]),
            calls: vec![tag_map(&[("env", "prod")])],
            expected: tag_map(&[("team", "net"), ("env", "prod")]),
        },
        TagRow {
            case: "no pairs at all: a call that changes nothing still has no content",
            target: Target::Monitor,
            initial: tag_map(&[("team", "net")]),
            calls: vec![tag_map(&[])],
            expected: tag_map(&[("team", "net")]),
        },
        TagRow {
            case: "a key the monitor already carries takes the new value",
            target: Target::Monitor,
            initial: tag_map(&[("env", "dev")]),
            calls: vec![tag_map(&[("env", "prod")])],
            expected: tag_map(&[("env", "prod")]),
        },
        TagRow {
            case: "eight pairs in one call",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![many_tags(8)],
            expected: many_tags(8),
        },
        TagRow {
            case: "successive calls, each answered the same way",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![tag_map(&[("a", "1")]), tag_map(&[("b", "2")])],
            expected: tag_map(&[("a", "1"), ("b", "2")]),
        },
        TagRow {
            case: "the same call repeated, the second one changing nothing",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![tag_map(&[("a", "1")]), tag_map(&[("a", "1")])],
            expected: tag_map(&[("a", "1")]),
        },
        TagRow {
            case: "an empty value, which the published TagValue minimum admits",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![tag_map(&[("owner", "")])],
            expected: tag_map(&[("owner", "")]),
        },
        TagRow {
            case: "a non-ASCII key and value",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![tag_map(&[("プロジェクト", "Ωmega")])],
            expected: tag_map(&[("プロジェクト", "Ωmega")]),
        },
        TagRow {
            case: "a key and a value at the published length maxima",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![tag_map(&[(longest_key.as_str(), longest_value.as_str())])],
            expected: tag_map(&[(longest_key.as_str(), longest_value.as_str())]),
        },
        TagRow {
            case: "the resource taken to the published TagMap maximum",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![many_tags(MAX_TAGS)],
            expected: many_tags(MAX_TAGS),
        },
        TagRow {
            case: "a probe, addressed by its own probe ARN",
            target: Target::Probe,
            initial: tag_map(&[("role", "edge")]),
            calls: vec![tag_map(&[("owner", "probe")])],
            expected: tag_map(&[("role", "edge"), ("owner", "probe")]),
        },
        TagRow {
            case: "a probe whose every stored key is overwritten",
            target: Target::Probe,
            initial: tag_map(&[("role", "edge"), ("owner", "probe")]),
            calls: vec![tag_map(&[("role", "core"), ("owner", "net")])],
            expected: tag_map(&[("role", "core"), ("owner", "net")]),
        },
    ];

    let router = router();
    for (index, row) in rows.into_iter().enumerate() {
        let TagRow {
            case,
            target,
            initial,
            calls,
            expected,
        } = row;
        let name = format!("nc-tag-{index}");
        let resource_arn = addressed_resource(&router, &name, target, &initial).await;

        for written in &calls {
            let (status, body) = tag_resource(&router, &resource_arn, written).await;
            assert_no_content(case, "TagResource", status, &body);
        }

        assert_tags_readable(&router, case, &resource_arn, &expected).await;
    }
}

/// One row of the `UntagResource` table.
struct UntagRow {
    case: &'static str,
    target: Target,
    initial: Value,
    /// The successive `UntagResource` calls this row makes.
    calls: Vec<Vec<String>>,
    expected: Value,
}

/// Every successful `UntagResource` answers 204 with no content, whatever keys it named
/// — including keys the resource does not carry, where it removes nothing.
#[tokio::test]
async fn a_successful_tag_removal_answers_no_content() {
    let longest_key = filler(MAX_TAG_KEY_LEN, 3);
    let full_set = many_tags(MAX_TAGS);

    let rows: Vec<UntagRow> = vec![
        UntagRow {
            case: "the recorded case: the one key named of the two a monitor carries",
            target: Target::Monitor,
            initial: tag_map(&[("team", "net"), ("env", "prod")]),
            calls: vec![keys(&["env"])],
            expected: tag_map(&[("team", "net")]),
        },
        UntagRow {
            case: "several keys in one call",
            target: Target::Monitor,
            initial: tag_map(&[("a", "1"), ("b", "2"), ("c", "3")]),
            calls: vec![keys(&["a", "c"])],
            expected: tag_map(&[("b", "2")]),
        },
        UntagRow {
            case: "a key the monitor does not carry removes nothing",
            target: Target::Monitor,
            initial: tag_map(&[("a", "1")]),
            calls: vec![keys(&["absent"])],
            expected: tag_map(&[("a", "1")]),
        },
        UntagRow {
            case: "the same removal repeated",
            target: Target::Monitor,
            initial: tag_map(&[("a", "1"), ("b", "2")]),
            calls: vec![keys(&["a"]), keys(&["a"])],
            expected: tag_map(&[("b", "2")]),
        },
        UntagRow {
            case: "every key, leaving the set empty",
            target: Target::Monitor,
            initial: tag_map(&[("a", "1"), ("b", "2")]),
            calls: vec![keys(&["a", "b"])],
            expected: tag_map(&[]),
        },
        UntagRow {
            case: "a monitor that carries no tag at all",
            target: Target::Monitor,
            initial: tag_map(&[]),
            calls: vec![keys(&["team"])],
            expected: tag_map(&[]),
        },
        UntagRow {
            case: "a key at the published TagKey length maximum",
            target: Target::Monitor,
            initial: tag_map(&[(longest_key.as_str(), "v"), ("keep", "me")]),
            calls: vec![vec![longest_key.clone()]],
            expected: tag_map(&[("keep", "me")]),
        },
        UntagRow {
            case: "the published TagMap maximum of keys named in one call",
            target: Target::Monitor,
            initial: full_set.clone(),
            calls: vec![keys_of(&full_set)],
            expected: tag_map(&[]),
        },
        UntagRow {
            case: "a probe, addressed by its own probe ARN",
            target: Target::Probe,
            initial: tag_map(&[("role", "edge"), ("owner", "probe")]),
            calls: vec![keys(&["role"])],
            expected: tag_map(&[("owner", "probe")]),
        },
        UntagRow {
            case: "every key of a probe, over two calls",
            target: Target::Probe,
            initial: tag_map(&[("role", "edge"), ("owner", "probe")]),
            calls: vec![keys(&["role"]), keys(&["owner"])],
            expected: tag_map(&[]),
        },
    ];

    let router = router();
    for (index, row) in rows.into_iter().enumerate() {
        let UntagRow {
            case,
            target,
            initial,
            calls,
            expected,
        } = row;
        let name = format!("nc-untag-{index}");
        let resource_arn = addressed_resource(&router, &name, target, &initial).await;

        for call_keys in &calls {
            let named: Vec<&str> = call_keys.iter().map(String::as_str).collect();
            let (status, body) = untag_resource(&router, &resource_arn, &named).await;
            assert_no_content(case, "UntagResource", status, &body);
        }

        assert_tags_readable(&router, case, &resource_arn, &expected).await;
    }
}

/// The boundary of the rule: a success that reports CONTENT keeps the protocol's
/// defaulted 200, and so do the two deletes, whose outputs also declare no members but
/// which the recorded AWS responses answer with 200.
#[tokio::test]
async fn a_success_outside_the_tag_mutation_class_keeps_the_defaulted_status() {
    let router = router();
    let (monitor_arn, _) = fixture(
        &router,
        "nc-boundary",
        &json!({"team": "net"}),
        &json!({"role": "edge"}),
    )
    .await;

    // Successes that report content: 200 WITH members to parse.
    let content_bearing: Vec<(&str, (u16, Value))> = vec![
        ("GetMonitor", get_monitor(&router, "nc-boundary").await),
        ("ListMonitors", call(&router, "GET", "/monitors", None).await),
        ("ListTagsForResource", list_tags(&router, &monitor_arn).await),
        (
            "UpdateMonitor",
            call(
                &router,
                "PATCH",
                "/monitors/nc-boundary",
                Some(r#"{"aggregationPeriod":60}"#),
            )
            .await,
        ),
    ];
    for (operation, (status, body)) in content_bearing {
        assert_eq!(
            status, OK,
            "case {operation:?}: a success that reports content answers {OK}, got {status} {body:?}"
        );
        assert!(
            body.as_object().is_some_and(|members| !members.is_empty()),
            "case {operation:?}: expected content in the response, got {body:?}"
        );
    }

    // The two deletes report nothing either, and keep the defaulted 200. A probe
    // is given up only once it has left `PENDING` and a monitor only once
    // nothing of it is still in service, so the probe is switched off first.
    let (_, read) = get_monitor(&router, "nc-boundary").await;
    let probe = only_probe(&read);
    let probe_id = probe
        .get("probeId")
        .and_then(Value::as_str)
        .expect("the fixture probe reports a probeId")
        .to_string();
    let (status, off) = deactivate_probe(&router, "nc-boundary", &probe_id).await;
    assert_eq!(
        status, 200,
        "setup: expected UpdateProbe 200 switching the probe off, got {status} {off:?}"
    );
    let empty_output: Vec<(&str, (u16, Value))> = vec![
        (
            "DeleteProbe",
            call(
                &router,
                "DELETE",
                &format!("/monitors/nc-boundary/probes/{probe_id}"),
                None,
            )
            .await,
        ),
        (
            "DeleteMonitor",
            call(&router, "DELETE", "/monitors/nc-boundary", None).await,
        ),
    ];
    for (operation, (status, body)) in empty_output {
        assert_eq!(
            status, OK,
            "case {operation:?}: an output with no members keeps the defaulted {OK}, \
             got {status} {body:?}"
        );
    }
}

/// 204 is a SUCCESS status: a refused tag mutation keeps the refusal's own status and
/// error code, over the whole class of refusals either operation raises.
#[tokio::test]
async fn a_refused_tag_mutation_keeps_its_own_status() {
    let router = router();
    let (monitor_arn, _) = fixture(
        &router,
        "nc-refusals",
        &json!({"team": "net"}),
        &json!({"role": "edge"}),
    )
    .await;

    // (case, resourceArn, status, error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a value outside the modelled Arn pattern",
            "not-an-arn-but-long-enough".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a well-formed monitor ARN naming no stored monitor",
            "arn:aws:networkmonitor:us-west-2:000000000000:monitor/nc-no-such-monitor".to_string(),
            404,
            TAG_NOT_FOUND_CODE,
        ),
        (
            "the stored monitor's ARN under another account",
            "arn:aws:networkmonitor:us-west-2:111122223333:monitor/nc-refusals".to_string(),
            404,
            TAG_NOT_FOUND_CODE,
        ),
    ];

    for (case, resource_arn, status, code) in cases {
        let (tagged, body) = tag_resource(&router, &resource_arn, &json!({"env": "prod"})).await;
        assert_eq!(
            (tagged, body.get("__type").and_then(Value::as_str)),
            (status, Some(code)),
            "case {case:?}: expected TagResource {status} {code}, got {tagged} {body:?}"
        );

        let (untagged, body) = untag_resource(&router, &resource_arn, &["team"]).await;
        assert_eq!(
            (untagged, body.get("__type").and_then(Value::as_str)),
            (status, Some(code)),
            "case {case:?}: expected UntagResource {status} {code}, got {untagged} {body:?}"
        );
    }

    // The refusals changed nothing: the stored set is the one it started with.
    assert_tags_readable(
        &router,
        "a refused mutation stores nothing",
        &monitor_arn,
        &json!({"team": "net"}),
    )
    .await;
}
