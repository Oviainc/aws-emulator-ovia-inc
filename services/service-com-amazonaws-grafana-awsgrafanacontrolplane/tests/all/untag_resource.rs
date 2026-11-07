// SPDX-License-Identifier: CC0-1.0

//! Removing a tag association from an Amazon Managed Grafana workspace, end to end
//! over HTTP.
//!
//! The rule under test: `UntagResource` removes from the resource its `resourceArn`
//! names exactly the tag keys `tagKeys` names — a key the resource carries is dropped,
//! a key it does not carry leaves the stored set as it was (the operation is
//! `@idempotent`), and every key the request does not name is kept. The readers
//! (`ListTagsForResource`, `DescribeWorkspace`, `ListWorkspaces`) then report the
//! remaining set. A `resourceArn` naming no workspace this store holds is a
//! `ResourceNotFoundException`, and one that names no Grafana workspace at all is a
//! `ValidationException`; neither removes anything.
//!
//! Every assertion goes through the router, so each case exercises routing, the query
//! and label bindings, the handler, the persisted state and serialization.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service; its region is the region the workspace
/// ARNs below are built for.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The canonical account id every emulated ARN carries (`emulator_account_id()`),
/// spelled out here rather than imported so a wrong account in the implementation
/// cannot make this test pass.
const ACCOUNT: &str = "000000000000";

/// `TagKey`'s published bounds, from
/// `smithy_query(query="shape", shape="TagKeys,TagKey")`: a `TagKeys` member is a
/// `TagKey` of length 1..128.
const MAX_KEY_LEN: usize = 128;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Percent-encode one path or query component, the way an AWS SDK does before putting
/// an ARN or a tag key on the wire: everything outside RFC 3986's unreserved set is
/// escaped.
fn encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The URI the tagging operations bind: `resourceArn` is a single, NON-greedy
/// `{resourceArn}` path segment of `/tags/{resourceArn}`.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", encode(arn))
}

/// The `UntagResource` URI: `tagKeys` is `@httpQuery`, so each key is its own repeated
/// `tagKeys=` query parameter.
fn untag_uri(arn: &str, keys: &[String]) -> String {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={}", encode(key))).collect();
    format!("{}?{}", tags_uri(arn), query.join("&"))
}

/// The ARN of a workspace in this account and region:
/// `arn:aws:grafana:<region>:<account>:/workspaces/<workspaceId>`, the form Amazon
/// Managed Grafana's resource ARNs carry. Spelled out here rather than imported.
fn workspace_arn(id: &str) -> String {
    format!("arn:aws:grafana:{REGION}:{ACCOUNT}:/workspaces/{id}")
}

/// Create a workspace carrying `tags` and answer its id.
async fn create_workspace(router: &MultiServiceRouter, tags: &BTreeMap<String, String>) -> String {
    let mut request = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    });
    if !tags.is_empty() {
        request["tags"] = json!(tags);
    }
    let created = call(router, "POST", "/workspaces", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "CreateWorkspace must succeed to set an untagging case up, got {} with {:?}",
        created.status, created.body
    );
    created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkspace answered no workspace id: {:?}", created.body))
        .to_string()
}

async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[String]) -> Answer {
    call(router, "DELETE", &untag_uri(arn, keys), None).await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &tags_uri(arn), None).await
}

/// The tag map at `value`, as an ordered map so two sets compare regardless of the
/// order the JSON object happened to carry.
fn tag_map(case: &str, what: &str, value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: {what} carried no tags object, got {value:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {what} tag {key:?} is not a string: {value:?}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected ListTagsForResource 200, got {} with {:?}",
        listed.status, listed.body
    );
    tag_map(case, "ListTagsForResource", &listed.body["tags"])
}

/// A tag set from literal pairs.
fn pairs(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| key.to_string()).collect()
}

/// One row of the removal table: the tags the workspace starts with, the key sets
/// submitted in order, and the tags the resource must then carry.
struct RemovalCase {
    name: &'static str,
    created_with: BTreeMap<String, String>,
    submissions: Vec<Vec<String>>,
    expected: BTreeMap<String, String>,
}

/// The rule, over a set of inputs differing in key count, how many of the submitted
/// keys the resource carries, the form of the keys, and the number of calls the keys
/// arrive in.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys_and_the_readers_report_the_rest() {
    let many: BTreeMap<String, String> = (0..12)
        .map(|i| (format!("key{i:02}"), format!("value{i:02}")))
        .collect();
    let longest_key = "k".repeat(MAX_KEY_LEN);

    let cases = vec![
        RemovalCase {
            name: "the only key the resource carries",
            created_with: pairs(&[("env", "dev")]),
            submissions: vec![keys(&["env"])],
            expected: BTreeMap::new(),
        },
        RemovalCase {
            name: "one of several keys, the others kept",
            created_with: pairs(&[("env", "dev"), ("team", "observability"), ("owner", "ana")]),
            submissions: vec![keys(&["team"])],
            expected: pairs(&[("env", "dev"), ("owner", "ana")]),
        },
        RemovalCase {
            name: "several keys in one call",
            created_with: pairs(&[("env", "dev"), ("team", "observability"), ("owner", "ana")]),
            submissions: vec![keys(&["env", "owner"])],
            expected: pairs(&[("team", "observability")]),
        },
        RemovalCase {
            name: "a key the resource does not carry removes nothing",
            created_with: pairs(&[("env", "dev")]),
            submissions: vec![keys(&["absent"])],
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "a mix of carried and absent keys drops only the carried ones",
            created_with: pairs(&[("env", "dev"), ("team", "observability")]),
            submissions: vec![keys(&["env", "absent"])],
            expected: pairs(&[("team", "observability")]),
        },
        RemovalCase {
            name: "the same key twice in one call",
            created_with: pairs(&[("env", "dev"), ("team", "observability")]),
            submissions: vec![keys(&["env", "env"])],
            expected: pairs(&[("team", "observability")]),
        },
        RemovalCase {
            name: "repeating the call is idempotent",
            created_with: pairs(&[("env", "dev"), ("team", "observability")]),
            submissions: vec![keys(&["env"]), keys(&["env"]), keys(&["env"])],
            expected: pairs(&[("team", "observability")]),
        },
        RemovalCase {
            name: "three calls, each naming a different key",
            created_with: pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            submissions: vec![keys(&["a"]), keys(&["b"]), keys(&["c"])],
            expected: pairs(&[("d", "4")]),
        },
        RemovalCase {
            name: "every key the resource carries, in one call",
            created_with: many.clone(),
            submissions: vec![many.keys().cloned().collect()],
            expected: BTreeMap::new(),
        },
        RemovalCase {
            name: "all but one of many keys",
            created_with: many.clone(),
            submissions: vec![many.keys().filter(|key| *key != "key07").cloned().collect()],
            expected: pairs(&[("key07", "value07")]),
        },
        RemovalCase {
            name: "a key whose value is the empty string is still a key",
            created_with: pairs(&[("note", ""), ("env", "dev")]),
            submissions: vec![keys(&["note"])],
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "non-ASCII keys",
            created_with: pairs(&[("équipe", "données"), ("チーム", "観測")]),
            submissions: vec![keys(&["équipe"])],
            expected: pairs(&[("チーム", "観測")]),
        },
        RemovalCase {
            name: "a key differing only in case is a different key",
            created_with: pairs(&[("Env", "dev"), ("env", "prod")]),
            submissions: vec![keys(&["env"])],
            expected: pairs(&[("Env", "dev")]),
        },
        RemovalCase {
            name: "the longest key the model admits",
            created_with: [(longest_key.clone(), "v".to_string()), ("env".to_string(), "dev".to_string())]
                .into_iter()
                .collect(),
            submissions: vec![vec![longest_key.clone()]],
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "a key carrying characters a client must percent-encode",
            created_with: pairs(&[("cost center/unit", "ops"), ("env", "dev")]),
            submissions: vec![keys(&["cost center/unit"])],
            expected: pairs(&[("env", "dev")]),
        },
    ];

    let router = emulator();
    for case in cases {
        let name = case.name;
        let id = create_workspace(&router, &case.created_with).await;
        let arn = workspace_arn(&id);
        assert_eq!(
            reported_tags(&router, name, &arn).await,
            case.created_with,
            "case {name:?}: the create must have stored the starting tags"
        );

        for (index, submission) in case.submissions.iter().enumerate() {
            let answer = untag(&router, &arn, submission).await;
            assert_eq!(
                answer.status, 200,
                "case {name:?}: UntagResource call {index} must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
        }

        let reported = reported_tags(&router, name, &arn).await;
        assert_eq!(
            reported, case.expected,
            "case {name:?}: ListTagsForResource must report the remaining tags, got {reported:?}"
        );

        // Round trip through the other reader of a workspace's tags.
        let described = call(&router, "GET", &format!("/workspaces/{id}"), None).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected DescribeWorkspace 200, got {} with {:?}",
            described.status, described.body
        );
        let on_workspace = tag_map(name, "DescribeWorkspace", &described.body["workspace"]["tags"]);
        assert_eq!(
            on_workspace, case.expected,
            "case {name:?}: the described workspace must carry the remaining tags, got {on_workspace:?}"
        );
    }
}

/// Tag, untag, tag again: the stored set after each call is what the readers report, so
/// a key removed by `UntagResource` can be associated again by `TagResource`.
#[tokio::test]
async fn a_removed_key_can_be_associated_again() {
    let case = "tag, untag, tag again";
    let router = emulator();
    let id = create_workspace(&router, &BTreeMap::new()).await;
    let arn = workspace_arn(&id);

    let tagged = call(
        &router,
        "POST",
        &tags_uri(&arn),
        Some(json!({"tags": {"env": "dev", "team": "observability"}})),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "case {case:?}: TagResource must answer 200, got {} with {:?}",
        tagged.status, tagged.body
    );

    let removed = untag(&router, &arn, &keys(&["env"])).await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: UntagResource must answer 200, got {} with {:?}",
        removed.status, removed.body
    );
    let after_removal = reported_tags(&router, case, &arn).await;
    assert_eq!(
        after_removal,
        pairs(&[("team", "observability")]),
        "case {case:?}: only the named key must be gone, got {after_removal:?}"
    );

    let retagged = call(
        &router,
        "POST",
        &tags_uri(&arn),
        Some(json!({"tags": {"env": "prod"}})),
    )
    .await;
    assert_eq!(
        retagged.status, 200,
        "case {case:?}: the second TagResource must answer 200, got {} with {:?}",
        retagged.status, retagged.body
    );
    let after_retag = reported_tags(&router, case, &arn).await;
    assert_eq!(
        after_retag,
        pairs(&[("env", "prod"), ("team", "observability")]),
        "case {case:?}: the removed key must be associable again, got {after_retag:?}"
    );
}

/// An ARN of the right form naming a workspace this store does not hold (never created,
/// or deleted) is a `ResourceNotFoundException` naming the workspace.
#[tokio::test]
async fn an_arn_naming_no_stored_workspace_is_a_resource_not_found() {
    let router = emulator();
    let created = create_workspace(&router, &pairs(&[("env", "dev")])).await;
    let deleted = call(&router, "DELETE", &format!("/workspaces/{created}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "DeleteWorkspace must succeed to set this case up, got {} with {:?}",
        deleted.status, deleted.body
    );
    // The delete is ACCEPTED, not finished: the workspace is removed from the store once
    // the removal the clock owes it falls due, and only then does its ARN name nothing.
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;

    for (case, id) in [
        ("a workspace id that was never minted", "g-ffffffffff".to_string()),
        ("a workspace that was deleted", created),
    ] {
        let arn = workspace_arn(&id);
        let answer = untag(&router, &arn, &keys(&["env"])).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected UntagResource 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["resourceId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the refusal must name the workspace, got {:?}",
            answer.body
        );
    }
}

/// `resourceArn` must name a Grafana workspace: the only resource this service tags.
/// Every other form is refused with the modelled `ValidationException`, and the refusal
/// removes nothing.
#[tokio::test]
async fn an_arn_that_names_no_grafana_workspace_is_refused_and_removes_nothing() {
    let router = emulator();
    let start = pairs(&[("env", "dev"), ("team", "observability")]);
    let id = create_workspace(&router, &start).await;

    let cases = vec![
        ("not an ARN at all", id.clone()),
        ("an ARN of another service", format!("arn:aws:s3:{REGION}:{ACCOUNT}:/workspaces/{id}")),
        (
            "a Grafana ARN naming another resource type",
            format!("arn:aws:grafana:{REGION}:{ACCOUNT}:/dashboards/{id}"),
        ),
        (
            "a workspaces ARN whose id is not a workspace id",
            format!("arn:aws:grafana:{REGION}:{ACCOUNT}:/workspaces/not-an-id"),
        ),
        (
            "a Grafana ARN with no resource section",
            format!("arn:aws:grafana:{REGION}:{ACCOUNT}:"),
        ),
        ("an ARN with too few fields", format!("arn:aws:grafana:/workspaces/{id}")),
    ];

    for (case, arn) in cases {
        let answer = untag(&router, &arn, &keys(&["env"])).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected UntagResource 400 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    let still = reported_tags(&router, "after the refusals", &workspace_arn(&id)).await;
    assert_eq!(
        still, start,
        "the refused calls must leave the workspace's tags untouched, got {still:?}"
    );
}

/// The members of `tagKeys` are `TagKey`s of length 1..128: a request naming a key
/// outside those published bounds is refused and removes nothing.
#[tokio::test]
async fn a_request_outside_the_tag_keys_constraints_is_refused_and_removes_nothing() {
    let router = emulator();
    let start = pairs(&[("env", "dev"), ("team", "observability")]);
    let id = create_workspace(&router, &start).await;
    let arn = workspace_arn(&id);

    let cases: Vec<(&str, String)> = vec![
        (
            "a key one character past the published maximum",
            untag_uri(&arn, &[format!("{}k", "k".repeat(MAX_KEY_LEN))]),
        ),
        ("a key shorter than the published minimum", untag_uri(&arn, &[String::new()])),
    ];

    for (case, uri) in cases {
        let answer = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let after = reported_tags(&router, case, &arn).await;
        assert_eq!(
            after, start,
            "case {case:?}: a refused request must remove nothing, got {after:?}"
        );
    }
}

/// Concurrent `UntagResource` calls, each naming a different key, all take effect: the
/// removal is from the stored map, not a replacement of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_untag_resource_calls_all_take_effect() {
    let case = "eight concurrent single-key calls";
    let router = Arc::new(emulator());
    let start: BTreeMap<String, String> = (0..8)
        .map(|i| (format!("concurrent{i}"), format!("value{i}")))
        .chain([("kept".to_string(), "yes".to_string())])
        .collect();
    let id = create_workspace(&router, &start).await;
    let arn = workspace_arn(&id);

    let mut running = Vec::new();
    for i in 0..8 {
        let router = Arc::clone(&router);
        let arn = arn.clone();
        running.push(tokio::spawn(async move {
            untag(&router, &arn, &[format!("concurrent{i}")]).await
        }));
    }
    for (index, task) in running.into_iter().enumerate() {
        let answer = task.await.expect("an untagging task must not panic");
        assert_eq!(
            answer.status, 200,
            "case {case:?}: concurrent call {index} must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let reported = reported_tags(&router, case, &arn).await;
    assert_eq!(
        reported,
        pairs(&[("kept", "yes")]),
        "case {case:?}: every concurrent removal must take effect, got {reported:?}"
    );
}
