// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's tag plane — `TagResource` and
//! `UntagResource` — read back through `ListTagsForResource`.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one input a
//! recording happened to carry:
//!
//! 1. `TagResource` MERGES the request's map into the map already stored on the graph:
//!    a key the graph does not hold is added, a key it already holds takes the
//!    request's value, and every key the request does not name keeps the value it had.
//!    Concurrent calls therefore lose no update.
//! 2. `UntagResource` removes exactly the keys the request names: a key the graph holds
//!    is dropped, a key it does not hold changes nothing — so the call is idempotent
//!    and a repeat answers the same success — and every key the request does not name
//!    is preserved.
//! 3. Both write operations resolve the graph first, so a graph the service does not
//!    hold is the modelled `ResourceNotFoundException` (404) carrying this service's
//!    fixed message, and nothing is written.
//! 4. Both carry the modelled success status 204 with no body.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    GRAPH_NOT_FOUND_MESSAGE, MAX_TAGS_PER_GRAPH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call, call_with, create_graph_ok, emulator, error_code, error_message, tags_of, ACCOUNT,
    REGION,
};

/// The modelled success status both tag writers carry
/// (`smithy_query(query="operation",operation="TagResource,UntagResource")`: `code` 204).
const TAG_WRITE_STATUS: u16 = 204;

/// Tag-key forms drawn from the modelled `TagKey` pattern
/// `^(?!aws:)[a-zA-Z+-=._:/]+$` (`smithy_query(query="shape",shape="TagKey")`), whose
/// `+-=` range spans `+ , - . / 0-9 : ; <  =` — so a key may carry punctuation and
/// digits, not only letters. None of these is the `Env`/`Team` pair the recordings
/// used.
///
/// `UntagResource` names its keys in the `tagKeys` query parameter, and every form
/// below reaches the handler verbatim there: the query reader splits pairs on `&` and
/// the first `=` only, and percent-decodes the rest. Two `TagKey` characters do NOT
/// survive a raw query — `+`, which that reader turns into a space, and `<`, which is
/// not a legal URI character at all — so a caller must percent-encode them, a
/// client-side encoding duty rather than a rule of this operation. Both are covered on
/// the assign path instead, by the `a-plus-in-the-key` and `an-angle-bracket-in-the-key`
/// rows of the merge table below.
const TAG_KEY_FORMS: &[&str] = &[
    "Letters",
    "cost.center",
    "team:security",
    "path/segment",
    "with-dash",
    "under_score",
    "digits123",
    "semi;colon",
    "comma,separated",
    "eq=sign",
];

/// A `GraphArn` that satisfies the modelled pattern and that nothing was ever stored
/// under.
fn never_created_arn(id_char: char) -> String {
    format!(
        "arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}",
        id_char.to_string().repeat(32)
    )
}

/// Owned key/value pairs from a literal table row.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Owned keys from a literal table row.
fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| (*key).to_string()).collect()
}

/// A table row's expected tag map, in the shape `ListTagsForResource` reports.
fn expected_tags(entries: &[(String, String)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect()
}

/// `TagResource` with `tags` on `graph_arn`.
async fn tag(
    router: &MultiServiceRouter,
    graph_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    let map: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect();
    call(
        router,
        &format!("/tags/{graph_arn}"),
        &json!({ "Tags": Value::Object(map) }),
    )
    .await
}

/// `UntagResource` naming `tag_keys` on `graph_arn`. `TagKeys` is bound to the repeated
/// `tagKeys` query parameter, so the keys go in the query string — see
/// [`TAG_KEY_FORMS`] for why every key this suite names survives there verbatim.
async fn untag(
    router: &MultiServiceRouter,
    graph_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    for key in tag_keys {
        assert!(
            !key.contains(['&', '+', '%', '<']),
            "tag key {key:?} needs percent-encoding before it can go in a raw query"
        );
    }
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    call_with(
        router,
        "DELETE",
        REGION,
        &format!("/tags/{graph_arn}?{query}"),
        &Value::Null,
    )
    .await
}

/// Assert a tag write answered the modelled 204 carrying no data: both outputs are
/// empty structures, so the answer holds no member whatever envelope it renders in.
fn assert_no_content(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case {case:?}: expected {TAG_WRITE_STATUS}, got {status} {body:?}"
    );
    let empty = match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: a {TAG_WRITE_STATUS} carries no member, got {body:?}"
    );
}

/// One row of the `UntagResource` table: what the graph holds, which keys the request
/// names, and what must remain.
struct UntagCase {
    name: String,
    stored: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// The `UntagResource` table. Rows differ along what the rule generalises over: how
/// many of the stored keys are named (one, some, all), whether a named key is stored at
/// all, the same key named twice, the character form of the keys, and the size of the
/// stored map (up to the per-resource tag quota).
fn untag_cases() -> Vec<UntagCase> {
    let three = pairs(&[("Env", "test"), ("Team", "sec"), ("Owner", "amy")]);

    // One key per modelled key form; the request removes every other one, so a form is
    // exercised both as a removed key and as a preserved one.
    let forms: Vec<(String, String)> = TAG_KEY_FORMS
        .iter()
        .enumerate()
        .map(|(n, key)| ((*key).to_string(), format!("value-{n}")))
        .collect();
    let forms_removed: Vec<String> = forms
        .iter()
        .step_by(2)
        .map(|(key, _)| key.clone())
        .collect();
    let forms_kept: Vec<(String, String)> = forms
        .iter()
        .skip(1)
        .step_by(2)
        .cloned()
        .collect();

    // A graph holding the per-resource tag quota's worth of keys, half of them removed.
    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_GRAPH)
        .map(|n| (format!("Key{n:02}"), format!("value-{n}")))
        .collect();
    let full_removed: Vec<String> = full
        .iter()
        .take(MAX_TAGS_PER_GRAPH / 2)
        .map(|(key, _)| key.clone())
        .collect();
    let full_kept: Vec<(String, String)> = full
        .iter()
        .skip(MAX_TAGS_PER_GRAPH / 2)
        .cloned()
        .collect();

    vec![
        UntagCase {
            name: "one-of-three".to_string(),
            stored: three.clone(),
            remove: keys(&["Env"]),
            expected: pairs(&[("Owner", "amy"), ("Team", "sec")]),
        },
        UntagCase {
            name: "two-of-three-preserves-the-third".to_string(),
            stored: three.clone(),
            remove: keys(&["Env", "Owner"]),
            expected: pairs(&[("Team", "sec")]),
        },
        UntagCase {
            name: "every-stored-key".to_string(),
            stored: three.clone(),
            remove: keys(&["Env", "Team", "Owner"]),
            expected: Vec::new(),
        },
        UntagCase {
            name: "only-a-key-the-graph-does-not-hold".to_string(),
            stored: three.clone(),
            remove: keys(&["NeverAssigned"]),
            expected: three.clone(),
        },
        UntagCase {
            name: "a-stored-key-beside-one-the-graph-does-not-hold".to_string(),
            stored: three.clone(),
            remove: keys(&["NeverAssigned", "Team"]),
            expected: pairs(&[("Env", "test"), ("Owner", "amy")]),
        },
        UntagCase {
            name: "a-reserved-key-the-graph-can-never-hold".to_string(),
            stored: three.clone(),
            remove: keys(&["aws:cloudformation:stack-name"]),
            expected: three.clone(),
        },
        UntagCase {
            name: "the-same-key-named-twice".to_string(),
            stored: three.clone(),
            remove: keys(&["Env", "Env"]),
            expected: pairs(&[("Owner", "amy"), ("Team", "sec")]),
        },
        UntagCase {
            name: "an-empty-valued-key".to_string(),
            stored: pairs(&[("Env", ""), ("Team", "sec")]),
            remove: keys(&["Env"]),
            expected: pairs(&[("Team", "sec")]),
        },
        UntagCase {
            name: "every-other-key-form".to_string(),
            stored: forms,
            remove: forms_removed,
            expected: forms_kept,
        },
        UntagCase {
            name: "half-of-a-graph-holding-the-tag-quota".to_string(),
            stored: full,
            remove: full_removed,
            expected: full_kept,
        },
    ]
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_the_request_names() {
    for case in untag_cases() {
        // A behavior graph is a per-account, per-Region singleton, so each row gets its
        // own emulator rather than reusing one graph.
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

        let (status, answered) = tag(&router, &graph_arn, &case.stored).await;
        assert_no_content(&format!("{}: TagResource", case.name), status, &answered);
        let stored = expected_tags(&case.stored);
        let visible = tags_of(&router, REGION, &graph_arn).await;
        assert_eq!(
            visible, stored,
            "case {:?}: expected the tags TagResource stored {stored:?}, got {visible:?}",
            case.name
        );

        let (status, answered) = untag(&router, &graph_arn, &case.remove).await;
        assert_no_content(&format!("{}: UntagResource", case.name), status, &answered);

        let expected = expected_tags(&case.expected);
        let visible = tags_of(&router, REGION, &graph_arn).await;
        assert_eq!(
            visible, expected,
            "case {:?}: expected the keys the request did not name {expected:?}, got {visible:?}",
            case.name
        );

        // Idempotent: removing a key the graph no longer holds is the same success and
        // the same map, so the second call changes nothing.
        let (status, answered) = untag(&router, &graph_arn, &case.remove).await;
        assert_no_content(
            &format!("{}: repeated UntagResource", case.name),
            status,
            &answered,
        );
        let visible = tags_of(&router, REGION, &graph_arn).await;
        assert_eq!(
            visible, expected,
            "case {:?}: a repeated removal must change nothing, expected {expected:?}, got {visible:?}",
            case.name
        );
    }
}

/// One row of the `TagResource` merge table: what the graph already holds, what the
/// request assigns, and the map that must result.
struct MergeCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    update: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

#[tokio::test]
async fn tag_resource_merges_into_the_map_the_graph_already_holds() {
    // Rows differ along the relation between the request's keys and the stored ones:
    // disjoint, overlapping, identical, and a re-assignment of the same value; and
    // along the value written (ordinary, empty, and the longest the modelled `TagValue`
    // allows).
    let cases = vec![
        MergeCase {
            name: "a-disjoint-key-is-added",
            initial: pairs(&[("Env", "test")]),
            update: pairs(&[("Team", "sec")]),
            expected: pairs(&[("Env", "test"), ("Team", "sec")]),
        },
        MergeCase {
            name: "a-held-key-takes-the-new-value",
            initial: pairs(&[("Env", "test"), ("Team", "sec")]),
            update: pairs(&[("Env", "prod")]),
            expected: pairs(&[("Env", "prod"), ("Team", "sec")]),
        },
        MergeCase {
            name: "overlapping-and-new-keys-together",
            initial: pairs(&[("Env", "test"), ("Team", "sec")]),
            update: pairs(&[("Env", "prod"), ("Owner", "amy")]),
            expected: pairs(&[("Env", "prod"), ("Owner", "amy"), ("Team", "sec")]),
        },
        MergeCase {
            name: "re-assigning-the-same-pair-changes-nothing",
            initial: pairs(&[("Env", "test")]),
            update: pairs(&[("Env", "test")]),
            expected: pairs(&[("Env", "test")]),
        },
        MergeCase {
            name: "a-value-can-be-emptied",
            initial: pairs(&[("Env", "test")]),
            update: pairs(&[("Env", "")]),
            expected: pairs(&[("Env", "")]),
        },
        MergeCase {
            // The one `TagKey` character the raw `tagKeys` query cannot carry (see
            // `TAG_KEY_FORMS`): the store takes it like any other key, which is what
            // this row pins.
            name: "a-plus-in-the-key",
            initial: pairs(&[("Env", "test")]),
            update: pairs(&[("plus+sign", "v")]),
            expected: pairs(&[("Env", "test"), ("plus+sign", "v")]),
        },
        MergeCase {
            // The other one (see `TAG_KEY_FORMS`): `<` is not a legal URI character, so
            // it is pinned here, where the key travels in the request body.
            name: "an-angle-bracket-in-the-key",
            initial: pairs(&[("Env", "test")]),
            update: pairs(&[("lt<sign", "v")]),
            expected: pairs(&[("Env", "test"), ("lt<sign", "v")]),
        },
        MergeCase {
            // `TagValue` length 0..256 (`smithy_query(query="shape",shape="TagValue")`).
            name: "the-longest-modelled-value",
            initial: pairs(&[("Env", "test")]),
            update: vec![("Env".to_string(), "v".repeat(256))],
            expected: vec![("Env".to_string(), "v".repeat(256))],
        },
    ];

    for case in cases {
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

        let (status, answered) = tag(&router, &graph_arn, &case.initial).await;
        assert_no_content(&format!("{}: first TagResource", case.name), status, &answered);
        let (status, answered) = tag(&router, &graph_arn, &case.update).await;
        assert_no_content(&format!("{}: second TagResource", case.name), status, &answered);

        let expected = expected_tags(&case.expected);
        let visible = tags_of(&router, REGION, &graph_arn).await;
        assert_eq!(
            visible, expected,
            "case {:?}: expected the merged map {expected:?}, got {visible:?}",
            case.name
        );
    }
}

/// Concurrency, run on a real thread pool so the calls genuinely overlap: whatever
/// order they land in, a merge may not drop another call's key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tag_resource_calls_lose_no_update() {
    const WRITERS: usize = 8;

    let router = Arc::new(emulator());
    let graph_arn = create_graph_ok(&router, REGION, &json!({ "Tags": { "Seed": "zero" } })).await;

    // Every writer assigns its own key, plus the one key they all contend on.
    let mut handles = Vec::new();
    for writer in 0..WRITERS {
        let router = Arc::clone(&router);
        let graph_arn = graph_arn.clone();
        handles.push(tokio::spawn(async move {
            let own = (format!("Writer{writer:02}"), format!("value-{writer}"));
            let shared = ("Contended".to_string(), format!("writer-{writer}"));
            tag(&router, &graph_arn, &[own, shared]).await
        }));
    }
    for (writer, handle) in handles.into_iter().enumerate() {
        let (status, answered) = handle.await.expect("the concurrent TagResource task");
        assert_no_content(&format!("concurrent writer {writer}"), status, &answered);
    }

    let visible = tags_of(&router, REGION, &graph_arn).await;
    assert_eq!(
        visible.len(),
        WRITERS + 2,
        "expected the seed key, the contended key and one key per writer, got {visible:?}"
    );
    assert_eq!(
        visible.get("Seed").and_then(Value::as_str),
        Some("zero"),
        "the key stored before the concurrent writes must survive them, got {visible:?}"
    );
    for writer in 0..WRITERS {
        let key = format!("Writer{writer:02}");
        assert_eq!(
            visible.get(&key).and_then(Value::as_str),
            Some(format!("value-{writer}").as_str()),
            "writer {writer}: its own key must survive every other writer, got {visible:?}"
        );
    }
    // The contended key holds exactly one of the written values — a last-writer-wins
    // value, never a missing or a blended one.
    let contended = visible
        .get("Contended")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the contended key was lost, got {visible:?}"));
    let written: Vec<String> = (0..WRITERS).map(|n| format!("writer-{n}")).collect();
    assert!(
        written.iter().any(|value| value == contended),
        "the contended key must hold one of {written:?}, got {contended:?}"
    );

    // Concurrent removals are the same rule in reverse: each writer's own key goes, the
    // keys nobody named stay.
    let mut handles = Vec::new();
    for writer in 0..WRITERS {
        let router = Arc::clone(&router);
        let graph_arn = graph_arn.clone();
        handles.push(tokio::spawn(async move {
            untag(&router, &graph_arn, &[format!("Writer{writer:02}")]).await
        }));
    }
    for (writer, handle) in handles.into_iter().enumerate() {
        let (status, answered) = handle.await.expect("the concurrent UntagResource task");
        assert_no_content(&format!("concurrent remover {writer}"), status, &answered);
    }

    let visible = tags_of(&router, REGION, &graph_arn).await;
    // `serde_json::Map` walks in insertion order wherever the build enables the crate's
    // `preserve_order` feature and in key order otherwise, so the WHICH-keys assertion
    // sorts rather than depending on either.
    let mut remaining: Vec<&str> = visible.keys().map(String::as_str).collect();
    remaining.sort_unstable();
    assert_eq!(
        remaining,
        vec!["Contended", "Seed"],
        "every writer's own key must be gone and no other key with it, got {visible:?}"
    );
}

#[tokio::test]
async fn untag_resource_on_a_graph_the_service_does_not_hold_is_the_modelled_404() {
    let router = emulator();

    // The rows differ along HOW the graph came to be absent, and along how many keys the
    // refused request named.
    let deleted = create_graph_ok(&router, REGION, &json!({ "Tags": { "Env": "test" } })).await;
    let (status, _) = call(&router, "/graph/removal", &json!({ "GraphArn": deleted })).await;
    assert_eq!(status, 200, "DeleteGraph on the graph about to be absent");

    for (how, arn) in [
        ("never-created", never_created_arn('b')),
        ("deleted", deleted),
    ] {
        for (shape, removed) in [
            ("one key", keys(&["Env"])),
            ("several keys", keys(&["Env", "Team", "Owner"])),
        ] {
            let case = format!("UntagResource naming {shape} on a {how} graph");
            let (status, answered) = untag(&router, &arn, &removed).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answered:?}"
            );
            let message = error_message(&answered);
            assert_eq!(
                message, GRAPH_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the fixed graph-not-found message, got {message:?}"
            );
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?}: the message must not echo the requested ARN, got {message:?}"
            );
        }
    }
}
