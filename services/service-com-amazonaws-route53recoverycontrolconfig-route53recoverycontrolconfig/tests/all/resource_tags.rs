// SPDX-License-Identifier: CC0-1.0

//! Tagging a recovery-control resource, end to end over HTTP.
//!
//! The rule under test: `TagResource` MERGES the request's `Tags` into the tag set stored
//! on the resource its `ResourceArn` names — a key the request carries takes the request's
//! value, every other key the resource already holds survives — `UntagResource` takes away
//! the keys `TagKeys` names and leaves the rest, and `ListTagsForResource` reports that
//! stored set back, for every kind of resource this service stores. An ARN no resource is
//! stored under is refused `ResourceNotFoundException` (404) with this service's fixed
//! not-found sentence, and one naming another account's resource `AccessDeniedException`
//! (403), decided before the lookup.
//!
//! The tables span what that rule generalises over: all four kinds of resource (cluster,
//! control panel — the default one a cluster is created with and a caller-created one —
//! routing control and safety rule), tags that ADD a key, tags that OVERWRITE one, several
//! keys at once, an empty map, removals of one key, of several, of a key the resource does
//! not carry and of every key left, and tag values differing in length (one character up to
//! the modelled maximum of 256) and in form.
//!
//! Every sentence and bound this suite expects is restated here, independently of the
//! service's own code.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, create_cluster, create_control_panel, create_routing_control, enc, error_code, router,
};

/// The account the emulator's own credential belongs to: the caller of every request here.
const CALLER_ACCOUNT: &str = "000000000000";

/// The IAM user that credential speaks for, as AWS names a principal in an authorization
/// refusal.
const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The namespace this service's IAM actions are published under: the model's `arnNamespace`
/// (`smithy_query(query="service_info")`).
const ACTION_NAMESPACE: &str = "route53-recovery-control-config";

/// The most tags one resource carries at once: AWS's tag restrictions state a maximum of 50
/// user-created tags per resource.
const MAX_TAGS: usize = 50;

/// The longest tag value the model admits: `__stringMin0Max256PatternS` is `length 0..256`
/// (`smithy_query(query="shape",shape="__mapOf__stringMin0Max256PatternS")`).
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The longest tag KEY a resource holds: AWS's tag restrictions state a maximum key length
/// of 128 characters (the model constrains the map's values, not its keys).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// A recovery-control ARN: AWS writes them with an empty Region segment.
fn arn(account: &str, tail: &str) -> String {
    format!("arn:aws:route53-recovery-control::{account}:{tail}")
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The fixed sentence this service refuses an ARN that resolves to nothing with.
fn not_found(noun: &str, requested_arn: &str) -> String {
    format!("No record corresponding to {noun} Arn({requested_arn})")
}

/// The sentence AWS refuses an unauthorized request with.
fn not_authorized(action: &str, resource: &str) -> String {
    format!(
        "User: {CALLER_PRINCIPAL} is not authorized to perform: {ACTION_NAMESPACE}:{action} \
         on resource: {resource} because no resource-based policy allows the \
         {ACTION_NAMESPACE}:{action} action"
    )
}

/// `TagResource`: `POST /tags/{ResourceArn}` with the tags in the body
/// (`smithy_query(query="operation",operation="TagResource")`).
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", enc(resource_arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// `ListTagsForResource`: `GET /tags/{ResourceArn}`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", enc(resource_arn)), None).await
}

/// `UntagResource`: `DELETE /tags/{ResourceArn}` with the keys as repeated `TagKeys` query
/// parameters (`smithy_query(query="operation",operation="UntagResource")`).
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("TagKeys={}", enc(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", enc(resource_arn)),
        None,
    )
    .await
}

/// The tags `ListTagsForResource` reports for `resource_arn`, as an ordered map.
async fn read_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "case list tags for {resource_arn}: expected 200, got {status} {body:?}"
    );
    as_map(&body["Tags"])
}

/// A JSON object as an ordered map, so two tag sets compare without depending on key order.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `(key, value)` pairs as the JSON object a `Tags` member carries.
fn tags_body(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// One resource of each kind this service stores, with the tags its create asked for.
struct Fixture {
    /// `(case label, the resource's ARN, the tags its create left on it)`.
    resources: Vec<(String, String, BTreeMap<String, String>)>,
}

/// Create one resource of every kind, three of them with `Tags` on the create call.
async fn fixture(router: &MultiServiceRouter, label: &str) -> Fixture {
    let cluster_tags = [("created-by", "cluster-create"), ("stage", "one")];
    let cluster = create_cluster(
        router,
        json!({
            "ClusterName": format!("{label}-cluster"),
            "Tags": tags_body(&cluster_tags)
        }),
    )
    .await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The panel AWS creates together with the cluster, which no create call of the
    // caller's tagged.
    let (status, listed) = call(
        router,
        "GET",
        &format!("/controlpanels?ClusterArn={}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list the cluster's panels: expected 200, got {status} {listed:?}"
    );
    let default_panel_arn = listed["ControlPanels"]
        .as_array()
        .and_then(|panels| {
            panels
                .iter()
                .find(|panel| panel["DefaultControlPanel"] == json!(true))
        })
        .and_then(|panel| panel["ControlPanelArn"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        !default_panel_arn.is_empty(),
        "case the cluster's default panel: expected one in {listed:?}"
    );

    let panel_tags = [("created-by", "panel-create")];
    let panel = create_control_panel(
        router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelName": format!("{label}-panel"),
            "Tags": tags_body(&panel_tags)
        }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let control = create_routing_control(
        router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": format!("{label}-control")
        }),
    )
    .await;
    let control_arn = control["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let rule_tags = [("created-by", "rule-create")];
    let (status, created) = call(
        router,
        "POST",
        "/safetyrule",
        Some(json!({
            "AssertionRule": {
                "AssertedControls": [control_arn],
                "ControlPanelArn": panel_arn,
                "Name": format!("{label}-rule"),
                "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "ATLEAST" },
                "WaitPeriodMs": 5000
            },
            "Tags": tags_body(&rule_tags)
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create the safety rule: expected 200, got {status} {created:?}"
    );
    let rule_arn = created["AssertionRule"]["SafetyRuleArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    Fixture {
        resources: vec![
            ("cluster".to_string(), cluster_arn, map(&cluster_tags)),
            (
                "default control panel".to_string(),
                default_panel_arn,
                BTreeMap::new(),
            ),
            ("control panel".to_string(), panel_arn, map(&panel_tags)),
            (
                "routing control".to_string(),
                control_arn,
                BTreeMap::new(),
            ),
            ("safety rule".to_string(), rule_arn, map(&rule_tags)),
        ],
    }
}

/// `(&str, &str)` pairs as an ordered map.
fn map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One `TagResource` call to make, and what it means for the resource's tag set.
struct TagCase {
    label: &'static str,
    tags: Vec<(String, String)>,
}

/// The tag requests to send, in order, against every kind of resource: a first key, a
/// second one, the first key again with a new value, two keys at once (one new, one
/// overwritten), the longest value the model admits, a value of one character, a key and
/// value carrying the punctuation the modelled pattern allows, and an empty map.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            label: "one new key",
            tags: vec![("env".to_string(), "prod".to_string())],
        },
        TagCase {
            label: "a second key; the first survives",
            tags: vec![("team".to_string(), "network".to_string())],
        },
        TagCase {
            label: "the first key again, with a new value",
            tags: vec![("env".to_string(), "staging".to_string())],
        },
        TagCase {
            label: "two keys at once, one new and one overwritten",
            tags: vec![
                ("env".to_string(), "dev".to_string()),
                ("owner".to_string(), "ops".to_string()),
            ],
        },
        TagCase {
            label: "a value of the modelled maximum length",
            tags: vec![("longest".to_string(), "v".repeat(MAX_TAG_VALUE_LENGTH))],
        },
        TagCase {
            label: "a value of one character",
            tags: vec![("shortest".to_string(), "v".to_string())],
        },
        TagCase {
            label: "a key and value carrying the punctuation the pattern admits",
            tags: vec![(
                "cost:center/42".to_string(),
                "a_b-c:d/e.f".to_string(),
            )],
        },
        TagCase {
            label: "an empty map, which changes nothing",
            tags: Vec::new(),
        },
    ]
}

/// RULE: `TagResource` merges its tags into the resource's stored set and
/// `ListTagsForResource` reports that set — for every kind of resource, over tags that add,
/// overwrite and change nothing, and without touching any other resource's tags.
#[tokio::test]
async fn tagging_merges_into_the_stored_set_and_is_read_back() {
    let router = router();
    let fixture = fixture(&router, "merge").await;

    // What each resource is expected to hold, computed here from the requests sent rather
    // than read back out of the service.
    let mut expected: Vec<BTreeMap<String, String>> = fixture
        .resources
        .iter()
        .map(|(_, _, created)| created.clone())
        .collect();

    // The tags each create left are already readable, before any tag call.
    for (index, (kind, resource_arn, _)) in fixture.resources.iter().enumerate() {
        let actual = read_tags(&router, resource_arn).await;
        assert_eq!(
            actual, expected[index],
            "case {kind} as created: expected {:?}, got {actual:?}",
            expected[index]
        );
    }

    for case in tag_cases() {
        for (index, (kind, resource_arn, _)) in fixture.resources.iter().enumerate() {
            let body = Value::Object(
                case.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            );
            let (status, answered) = tag(&router, resource_arn, &body).await;
            assert_eq!(
                status, 200,
                "case {kind} / {}: expected TagResource 200, got {status} {answered:?}",
                case.label
            );

            for (key, value) in &case.tags {
                expected[index].insert(key.clone(), value.clone());
            }
            let actual = read_tags(&router, resource_arn).await;
            assert_eq!(
                actual, expected[index],
                "case {kind} / {}: expected {:?}, got {actual:?}",
                case.label, expected[index]
            );
        }

        // Tagging one resource never writes another's set: every resource still holds
        // exactly what the requests addressed to IT asked for.
        for (index, (kind, resource_arn, _)) in fixture.resources.iter().enumerate() {
            let actual = read_tags(&router, resource_arn).await;
            assert_eq!(
                actual, expected[index],
                "case {kind} after {}: another resource's tag call must not change it, \
                 expected {:?}, got {actual:?}",
                case.label, expected[index]
            );
        }
    }
}

/// RULE at its removal end: `UntagResource` takes away the keys `TagKeys` names and leaves
/// every other key on the resource, so the tag set a later `ListTagsForResource` reports is
/// the stored one minus exactly those keys — for every kind of resource.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    let router = router();
    let fixture = fixture(&router, "remove").await;

    // The set every resource is brought to first, so the rows below start from one state.
    let seeded = [
        ("env", "prod"),
        ("team", "network"),
        ("owner", "ops"),
        ("cost:center/42", "a_b-c"),
    ];

    // Rows: one key, a key the resource does not carry, the same key twice in one request,
    // two keys at once, and every key that is left.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("one key", vec!["env"]),
        ("a key the resource does not carry", vec!["absent"]),
        ("the same key twice in one request", vec!["team", "team"]),
        (
            "two keys at once, one of them already removed",
            vec!["team", "owner"],
        ),
        (
            "every key that is left, including those its create set",
            vec!["cost:center/42", "created-by", "stage"],
        ),
        ("a key of a resource that is already untagged", vec!["env"]),
    ];

    for (kind, resource_arn, _) in &fixture.resources {
        let (status, answered) = tag(&router, resource_arn, &tags_body(&seeded)).await;
        assert_eq!(
            status, 200,
            "case {kind} seeded: expected TagResource 200, got {status} {answered:?}"
        );
        let mut expected = read_tags(&router, resource_arn).await;

        for (case, keys) in &rows {
            let (status, answered) = untag(&router, resource_arn, keys).await;
            assert_eq!(
                status, 200,
                "case {kind} / {case}: expected UntagResource 200, got {status} {answered:?}"
            );
            for key in keys {
                expected.remove(*key);
            }
            let actual = read_tags(&router, resource_arn).await;
            assert_eq!(
                actual, expected,
                "case {kind} / {case}: expected {expected:?}, got {actual:?}"
            );
        }

        assert!(
            expected.is_empty(),
            "case {kind}: the rows remove every seeded key, so the resource ends untagged; \
             expected an empty set, got {expected:?}"
        );
    }
}

/// RULE, at its read end: `ListTagsForResource` is a pure read, so reading twice answers
/// the same set and reading an untagged resource creates nothing.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let router = router();
    let fixture = fixture(&router, "pure-read").await;

    for (kind, resource_arn, created) in &fixture.resources {
        let first = read_tags(&router, resource_arn).await;
        let second = read_tags(&router, resource_arn).await;
        assert_eq!(
            first, *created,
            "case {kind}: expected the tags its create left, {created:?}, got {first:?}"
        );
        assert_eq!(
            second, first,
            "case {kind} read twice: expected {first:?}, got {second:?}"
        );
    }
}

/// RULE at its bound: a resource carries at most `MAX_TAGS` tags, so a request that would
/// leave it holding more is refused `ValidationException` (400) and writes nothing.
#[tokio::test]
async fn a_request_past_the_tag_ceiling_is_refused_and_writes_nothing() {
    let router = router();
    let fixture = fixture(&router, "ceiling").await;
    let (kind, resource_arn, created) = &fixture.resources[0];

    // Fill the resource to exactly the ceiling, replacing what its create left.
    let mut expected: BTreeMap<String, String> = created.clone();
    let room = MAX_TAGS - expected.len();
    let fill: Vec<(String, String)> = (0..room)
        .map(|n| (format!("fill-{n}"), format!("value-{n}")))
        .collect();
    let body = Value::Object(
        fill.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    );
    let (status, answered) = tag(&router, resource_arn, &body).await;
    assert_eq!(
        status, 200,
        "case {kind} filled to {MAX_TAGS}: expected 200, got {status} {answered:?}"
    );
    expected.extend(fill);
    let actual = read_tags(&router, resource_arn).await;
    assert_eq!(
        actual.len(),
        MAX_TAGS,
        "case {kind} at the ceiling: expected {MAX_TAGS} tags, got {}",
        actual.len()
    );

    // Rows: one key past the ceiling, and a whole batch past it. Re-tagging a key the
    // resource already holds is NOT past it, however many are sent.
    let rows: Vec<(&str, Value, Option<usize>)> = vec![
        (
            "one key past the ceiling",
            json!({ "one-too-many": "x" }),
            Some(MAX_TAGS + 1),
        ),
        (
            "five keys past the ceiling",
            json!({ "a": "1", "b": "2", "c": "3", "d": "4", "e": "5" }),
            Some(MAX_TAGS + 5),
        ),
        (
            "every key it already holds, with new values",
            Value::Object(
                actual
                    .keys()
                    .map(|key| (key.clone(), Value::String("rewritten".to_string())))
                    .collect(),
            ),
            None,
        ),
    ];

    for (case, body, refused_at) in rows {
        let (status, answer) = tag(&router, resource_arn, &body).await;
        match refused_at {
            Some(resulting) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected 400, got {status} {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {case}: expected ValidationException, got {answer:?}"
                );
                let expected_message = format!(
                    "A resource may carry at most {MAX_TAGS} tags; this request would leave it \
                     with {resulting}."
                );
                let actual_message = message(&answer);
                assert_eq!(
                    actual_message, expected_message,
                    "case {case}: expected {expected_message:?}, got {actual_message:?}"
                );
                let after = read_tags(&router, resource_arn).await;
                assert_eq!(
                    after, expected,
                    "case {case}: a refused request writes nothing, expected {expected:?}, \
                     got {after:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: re-tagging keys it already holds stays within the ceiling, \
                     got {status} {answer:?}"
                );
                for key in expected.clone().keys() {
                    expected.insert(key.clone(), "rewritten".to_string());
                }
                let after = read_tags(&router, resource_arn).await;
                assert_eq!(
                    after, expected,
                    "case {case}: expected {expected:?}, got {after:?}"
                );
            }
        }
    }
}

/// RULE at its other bound: a tag key is at most `MAX_TAG_KEY_LENGTH` characters, so a key
/// at that bound is stored and one past it is refused `ValidationException` (400) with
/// nothing written.
#[tokio::test]
async fn a_tag_key_past_its_length_bound_is_refused_and_writes_nothing() {
    let router = router();
    let cluster = create_cluster(&router, json!({ "ClusterName": "key-length" })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: one character, the published bound, one past it and well past it.
    let rows: Vec<(String, usize)> = vec![
        ("one character".to_string(), 1),
        ("the published bound".to_string(), MAX_TAG_KEY_LENGTH),
        (
            "one character past the bound".to_string(),
            MAX_TAG_KEY_LENGTH + 1,
        ),
        (
            "far past the bound".to_string(),
            MAX_TAG_KEY_LENGTH * 10,
        ),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (case, length) in rows {
        let key = "k".repeat(length);
        let (status, answer) = tag(&router, &cluster_arn, &json!({ key.clone(): "v" })).await;
        if length <= MAX_TAG_KEY_LENGTH {
            assert_eq!(
                status, 200,
                "case a key of {length} characters ({case}): expected 200, got {status} {answer:?}"
            );
            expected.insert(key, "v".to_string());
        } else {
            assert_eq!(
                status, 400,
                "case a key of {length} characters ({case}): expected 400, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case a key of {length} characters ({case}): expected ValidationException, got \
                 {answer:?}"
            );
            let expected_message = format!(
                "A tag key may be at most {MAX_TAG_KEY_LENGTH} characters; this request carries \
                 one of {length}."
            );
            let actual_message = message(&answer);
            assert_eq!(
                actual_message, expected_message,
                "case a key of {length} characters ({case}): expected {expected_message:?}, got \
                 {actual_message:?}"
            );
        }
        let actual = read_tags(&router, &cluster_arn).await;
        assert_eq!(
            actual, expected,
            "case a key of {length} characters ({case}): expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: an ARN that resolves to no stored resource is refused `ResourceNotFoundException`
/// (404) with the fixed not-found sentence, naming the kind the ARN itself declares and
/// echoing the ARN the request sent — on both tagging operations.
#[tokio::test]
async fn an_arn_resolving_to_nothing_is_refused_with_the_no_record_sentence() {
    let router = router();
    // A tagged resource exists throughout, so a refusal is about the ARN the request names
    // and not about an empty store.
    let fixture = fixture(&router, "absent").await;
    let (_, present_arn, _) = &fixture.resources[0];
    assert!(
        !read_tags(&router, present_arn).await.is_empty(),
        "case the present resource: it carries the tags its create asked for"
    );

    // Several fillings of a resource id, each satisfying the pattern the model puts on this
    // service's ARN members (`^[A-Za-z0-9:\/_-]*$`).
    let ids = [
        "00000000-0000-0000-0000-000000000000",
        "7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4",
        "ABCDEF01-2345-6789-ABCD-EF0123456789",
        "named_resource-1",
    ];

    let mut rows: Vec<(String, &'static str)> = Vec::new();
    for id in ids {
        let panel = arn(CALLER_ACCOUNT, &format!("controlpanel/{id}"));
        rows.push((arn(CALLER_ACCOUNT, &format!("cluster/{id}")), "cluster"));
        rows.push((panel.clone(), "control panel"));
        rows.push((
            format!("{panel}/routingcontrol/{id}"),
            "routing control",
        ));
        rows.push((format!("{panel}/safetyrule/{id}"), "safety rule"));
    }
    // A value that names no resource kind of this service at all: the sentence names the
    // kind-agnostic noun.
    rows.push(("not-an-arn".to_string(), "resource"));
    rows.push((
        arn(CALLER_ACCOUNT, "somethingelse/abc"),
        "resource",
    ));

    for (resource_arn, noun) in &rows {
        for (operation, status_and_answer) in [
            ("TagResource", tag(&router, resource_arn, &json!({ "k": "v" })).await),
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            ("UntagResource", untag(&router, resource_arn, &["k"]).await),
        ] {
            let (status, answer) = status_and_answer;
            assert_eq!(
                status, 404,
                "case {operation}({resource_arn}): expected 404, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {operation}({resource_arn}): expected ResourceNotFoundException, got \
                 {answer:?}"
            );
            let actual = message(&answer);
            let expected = not_found(noun, resource_arn);
            assert_eq!(
                actual, expected,
                "case {operation}({resource_arn}): expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// RULE: the ARN in that sentence is the one the request carried, so a resource this
/// service minted, tagged and then deleted is answered for by the 404 and its tags are
/// gone with it.
#[tokio::test]
async fn a_deleted_resource_takes_its_tags_with_it() {
    let router = router();
    let cluster = create_cluster(&router, json!({ "ClusterName": "deleted-tags" })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, answered) = tag(&router, &cluster_arn, &json!({ "env": "prod" })).await;
    assert_eq!(
        status, 200,
        "case tag the cluster: expected 200, got {status} {answered:?}"
    );
    let stored = read_tags(&router, &cluster_arn).await;
    assert_eq!(
        stored,
        map(&[("env", "prod")]),
        "case tag the cluster: expected the tag back, got {stored:?}"
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete the cluster: expected 200, got {status} {deleted:?}"
    );

    let (status, gone) = list_tags(&router, &cluster_arn).await;
    assert_eq!(
        status, 404,
        "case list the deleted cluster's tags: expected 404, got {status} {gone:?}"
    );
    let actual = message(&gone);
    assert_eq!(
        actual,
        not_found("cluster", &cluster_arn),
        "case list the deleted cluster's tags: expected the minted ARN echoed, got {actual:?}"
    );
}

/// RULE: an ARN of another account is refused `AccessDeniedException` (403) naming the
/// principal, the action and the ARN, before any lookup — on both tagging operations.
#[tokio::test]
async fn an_arn_of_another_account_is_refused_with_access_denied() {
    let router = router();
    let fixture = fixture(&router, "foreign").await;

    // Rows: an ARN of each kind in three foreign accounts, and every ARN this service
    // really minted with only its account swapped — so the refusal is demonstrably not
    // about the resource being absent.
    let mut rows: Vec<String> = Vec::new();
    for account in ["123456789012", "999999999999", "210987654321"] {
        let panel = arn(account, "controlpanel/7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4");
        rows.push(arn(
            account,
            "cluster/7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4",
        ));
        rows.push(panel.clone());
        rows.push(format!("{panel}/routingcontrol/abc"));
        rows.push(format!("{panel}/safetyrule/abc"));
    }
    for (_, resource_arn, _) in &fixture.resources {
        rows.push(resource_arn.replace(CALLER_ACCOUNT, "123456789012"));
    }

    for resource_arn in &rows {
        for (operation, status_and_answer) in [
            ("TagResource", tag(&router, resource_arn, &json!({ "k": "v" })).await),
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            ("UntagResource", untag(&router, resource_arn, &["k"]).await),
        ] {
            let (status, answer) = status_and_answer;
            assert_eq!(
                status, 403,
                "case {operation}({resource_arn}): expected 403, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {operation}({resource_arn}): expected AccessDeniedException, got {answer:?}"
            );
            let actual = message(&answer);
            let expected = not_authorized(operation, resource_arn);
            assert_eq!(
                actual, expected,
                "case {operation}({resource_arn}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    // And none of those refusals wrote anything: every resource still holds the tags its
    // create asked for.
    for (kind, resource_arn, created) in &fixture.resources {
        let actual = read_tags(&router, resource_arn).await;
        assert_eq!(
            actual, *created,
            "case {kind} after the refusals: expected {created:?}, got {actual:?}"
        );
    }
}
