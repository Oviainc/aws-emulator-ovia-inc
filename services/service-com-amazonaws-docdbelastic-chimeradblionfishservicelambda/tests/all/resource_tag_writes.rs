// SPDX-License-Identifier: CC0-1.0

//! Writing a resource's tags over HTTP, for every kind of resource this service tags.
//!
//! The rule these suites assert: **a tagging write edits the tag map persisted on the
//! record its `resourceArn` names, and nothing else** — `TagResource` MERGES the map it
//! carries into the stored one (a key the resource does not carry is added, a key it
//! already carries takes the request's value, every key the request does not name keeps
//! the value it had), `UntagResource` removes exactly the keys it names and is a no-op
//! for a key the resource does not carry, and a write against an ARN this emulator
//! holds no resource under is refused 404 `ResourceNotFoundException` — whose
//! `resourceType` follows the ARN's own resource segment — with nothing persisted.
//!
//! The rule generalises over the kind of resource, over the SIZE of the map written and
//! over the FORM of its keys and values, so the tables below span both taggable kinds
//! and rows that differ along count (none, one, several, up to the published maximum),
//! along whether a key is new or already present, and along key/value form (every
//! punctuation character `TagKey` admits, an empty value, a key and a value at their
//! published maximum length). The recorded case — one tag written onto a snapshot and
//! then removed again — is one row of it.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The AWS resource type NAMES this service reports in `resourceType`.
///
/// Source: the resource class each operation reads and writes one of, as its own
/// members name it — `GetCluster` answers `cluster` and is addressed by `clusterArn`,
/// `GetClusterSnapshot` answers `snapshot` and is addressed by `snapshotArn`
/// (`smithy_query(query="operation", operation="GetCluster,GetClusterSnapshot")`).
const CLUSTER_TYPE: &str = "Cluster";
const SNAPSHOT_TYPE: &str = "Snapshot";

/// The greatest number of tags one resource carries.
///
/// Source: `UntagResourceInput$tagKeys` is modelled `length(min: 0, max: 50)`
/// (`smithy_query(query="operation", operation="UntagResource")`), so no resource holds
/// more than 50 distinct tag keys.
const MAX_TAGS: usize = 50;

/// The published length bounds of a tag key and of a tag value.
///
/// Source: `smithy_query(query="shape", shape="TagKey,TagValue")` — `TagKey` is
/// length 1..128, `TagValue` length 0..256.
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// Punctuation a tag key may carry besides letters and digits.
///
/// Source: the `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// (`smithy_query(query="shape", shape="TagKey")`), whose character class spans `+`
/// through `=` — so `, - . / 0-9 : ; < =` — plus `_` and `.`.
const TAG_KEY_PUNCTUATION: &[char] = &['+', ',', '-', '.', '/', ':', ';', '<', '=', '_'];

/// A SigV4 `Authorization` header for this service in `region`.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: the shared engine plus a router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one value, the way an AWS SDK does for a single path segment or one
/// query-parameter value: an ARN carries both `:` and `/` and a tag key carries
/// punctuation, so nothing outside the unreserved set survives raw.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when the
/// body is empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
        .header("authorization", authorization(REGION))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`, the reader every write is proved
/// through.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", encode(arn)), None).await
}

/// `POST /tags/{resourceArn}` — `TagResource`, whose `tags` map travels in the body.
async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(arn)),
        Some(json!({"tags": tag_object(tags)})),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
/// `@httpQuery` list, so each key travels as its own repeated `tagKeys=` parameter.
async fn untag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    keys: &[String],
) -> (u16, Value) {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={}", encode(key))).collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", encode(arn), query.join("&")),
        None,
    )
    .await
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A tag map as a JSON object.
fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    let mut object = Map::new();
    for (key, value) in tags {
        object.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(object)
}

/// The `tags` member of a `ListTagsForResource` response, as a comparable map.
fn reported_tags(body: &Value) -> Option<BTreeMap<String, String>> {
    let object = body.get("tags")?.as_object()?;
    Some(
        object
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect(),
    )
}

/// The tag map persisted on `arn`, read back through `ListTagsForResource`.
async fn persisted_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the tags back is HTTP 200, got {status} with {body}"
    );
    reported_tags(&body)
        .unwrap_or_else(|| panic!("case {case:?}: the response carries a tags map, got {body}"))
}

/// A map built from `(key, value)` pairs.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A list of owned keys.
fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// Create a cluster carrying `tags` and return its ARN.
async fn create_cluster(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let body = json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
        "tags": tag_object(tags),
    });
    let (status, created) = call(router, "POST", "/cluster", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating cluster {name:?} must succeed, got {status} with {created}"
    );
    created["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created cluster carries its ARN"))
        .to_string()
}

/// Snapshot `cluster_arn` with `tags` and return the snapshot's ARN.
async fn create_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    cluster_arn: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let body = json!({
        "clusterArn": cluster_arn,
        "snapshotName": name,
        "tags": tag_object(tags),
    });
    let (status, created) = call(router, "POST", "/cluster-snapshot", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating snapshot {name:?} must succeed, got {status} with {created}"
    );
    created["snapshot"]["snapshotArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created snapshot carries its ARN"))
        .to_string()
}

/// One resource of each taggable kind, both carrying `initial`, as
/// `(kind, arn)` pairs. `index` keeps each row's resources distinct.
async fn taggable_resources(
    router: &MultiServiceRouter,
    case: &str,
    index: usize,
    initial: &BTreeMap<String, String>,
) -> Vec<(&'static str, String)> {
    let cluster_arn = create_cluster(
        router,
        case,
        &format!("tagwrite-cluster-{index}"),
        initial,
    )
    .await;
    let snapshot_arn = create_snapshot(
        router,
        case,
        &format!("tagwrite-snapshot-{index}"),
        &cluster_arn,
        initial,
    )
    .await;
    vec![(CLUSTER_TYPE, cluster_arn), (SNAPSHOT_TYPE, snapshot_arn)]
}

/// One row of the merge table: the map the resource is created with, the map one
/// `TagResource` call writes, and the map the resource must carry afterwards.
struct MergeCase {
    case: &'static str,
    initial: BTreeMap<String, String>,
    written: BTreeMap<String, String>,
    expected: BTreeMap<String, String>,
}

/// Writes differing along count, along whether the keys are new or already present, and
/// along the form of the keys and values written.
fn merge_cases() -> Vec<MergeCase> {
    // Every punctuation character the TagKey pattern admits, in one key.
    let punctuation_key: String = TAG_KEY_PUNCTUATION.iter().collect();
    // The published maxima, which an entry at the bound must survive.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    // A resource one key short of the published maximum, and the key that fills it.
    let nearly_full: BTreeMap<String, String> = (0..MAX_TAGS - 1)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    let mut filled = nearly_full.clone();
    filled.insert("last".to_string(), "entry".to_string());

    vec![
        MergeCase {
            case: "one tag onto a resource carrying none",
            initial: BTreeMap::new(),
            written: map(&[("Project", "lionfish")]),
            expected: map(&[("Project", "lionfish")]),
        },
        MergeCase {
            case: "several new tags beside the ones already stored",
            initial: map(&[("Project", "lionfish")]),
            written: map(&[("Environment", "staging"), ("Owner", "platform")]),
            expected: map(&[
                ("Environment", "staging"),
                ("Owner", "platform"),
                ("Project", "lionfish"),
            ]),
        },
        MergeCase {
            case: "a key already present takes the new value",
            initial: map(&[("Project", "lionfish"), ("Tier", "gold")]),
            written: map(&[("Project", "stonefish")]),
            expected: map(&[("Project", "stonefish"), ("Tier", "gold")]),
        },
        MergeCase {
            case: "every stored key overwritten at once",
            initial: map(&[("Project", "lionfish"), ("Tier", "gold")]),
            written: map(&[("Project", "stonefish"), ("Tier", "silver")]),
            expected: map(&[("Project", "stonefish"), ("Tier", "silver")]),
        },
        MergeCase {
            case: "an existing key overwritten with an empty value",
            initial: map(&[("Project", "lionfish")]),
            written: map(&[("Project", "")]),
            expected: map(&[("Project", "")]),
        },
        MergeCase {
            case: "a key carrying every admitted punctuation character",
            initial: map(&[("plain", "value")]),
            written: [(punctuation_key.clone(), "punctuated".to_string())]
                .into_iter()
                .collect(),
            expected: [
                ("plain".to_string(), "value".to_string()),
                (punctuation_key, "punctuated".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        MergeCase {
            case: "a key and a value at their published maximum length",
            initial: map(&[("short", "s")]),
            written: [(longest_key.clone(), longest_value.clone())]
                .into_iter()
                .collect(),
            expected: [
                ("short".to_string(), "s".to_string()),
                (longest_key, longest_value),
            ]
            .into_iter()
            .collect(),
        },
        MergeCase {
            case: "the key that fills the published maximum number of tags",
            initial: nearly_full,
            written: map(&[("last", "entry")]),
            expected: filled,
        },
    ]
}

#[tokio::test]
async fn tagging_merges_the_written_map_into_the_persisted_one() {
    let router = emulator();

    for (index, row) in merge_cases().into_iter().enumerate() {
        let resources = taggable_resources(&router, row.case, index, &row.initial).await;
        for (kind, arn) in resources {
            let case = format!("{} / {kind}", row.case);

            let (status, body) = tag_resource(&router, &arn, &row.written).await;
            assert_eq!(
                status, 200,
                "case {case:?}: tagging an existing resource is HTTP 200, got {status} \
                 with {body}"
            );

            let stored = persisted_tags(&router, &case, &arn).await;
            assert_eq!(
                stored, row.expected,
                "case {case:?}: expected the merged tag map ({} entries), got {stored:?}",
                row.expected.len()
            );
        }
    }
}

/// One row of the removal table: the map the resource starts with, the keys one
/// `UntagResource` call names, and the map the resource must carry afterwards.
struct RemovalCase {
    case: &'static str,
    initial: BTreeMap<String, String>,
    removed: Vec<String>,
    expected: BTreeMap<String, String>,
}

/// Removals differing along how many keys are named, whether each is present, and the
/// form of the keys.
fn removal_cases() -> Vec<RemovalCase> {
    let punctuation_key: String = TAG_KEY_PUNCTUATION.iter().collect();
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    // A resource at the published maximum, emptied in one call.
    let full: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    let all_keys: Vec<String> = full.keys().cloned().collect();

    vec![
        RemovalCase {
            case: "one key of several",
            initial: map(&[
                ("Project", "lionfish"),
                ("Environment", "staging"),
                ("Owner", "platform"),
            ]),
            removed: keys(&["Environment"]),
            expected: map(&[("Owner", "platform"), ("Project", "lionfish")]),
        },
        RemovalCase {
            case: "the only key the resource carries",
            initial: map(&[("Project", "lionfish")]),
            removed: keys(&["Project"]),
            expected: BTreeMap::new(),
        },
        RemovalCase {
            case: "several keys at once, leaving one",
            initial: map(&[
                ("Project", "lionfish"),
                ("Environment", "staging"),
                ("Owner", "platform"),
            ]),
            removed: keys(&["Environment", "Owner"]),
            expected: map(&[("Project", "lionfish")]),
        },
        RemovalCase {
            case: "a key the resource does not carry",
            initial: map(&[("Project", "lionfish")]),
            removed: keys(&["NeverSet"]),
            expected: map(&[("Project", "lionfish")]),
        },
        RemovalCase {
            case: "a present key named beside an absent one",
            initial: map(&[("Project", "lionfish"), ("Tier", "gold")]),
            removed: keys(&["Tier", "NeverSet"]),
            expected: map(&[("Project", "lionfish")]),
        },
        RemovalCase {
            case: "a key whose value is empty",
            initial: map(&[("Placeholder", ""), ("Project", "lionfish")]),
            removed: keys(&["Placeholder"]),
            expected: map(&[("Project", "lionfish")]),
        },
        RemovalCase {
            case: "a key carrying every admitted punctuation character",
            initial: [
                (punctuation_key.clone(), "punctuated".to_string()),
                ("plain".to_string(), "value".to_string()),
            ]
            .into_iter()
            .collect(),
            removed: vec![punctuation_key],
            expected: map(&[("plain", "value")]),
        },
        RemovalCase {
            case: "a key at its published maximum length",
            initial: [
                (longest_key.clone(), "long".to_string()),
                ("short".to_string(), "s".to_string()),
            ]
            .into_iter()
            .collect(),
            removed: vec![longest_key],
            expected: map(&[("short", "s")]),
        },
        RemovalCase {
            case: "every key of a resource at the published maximum",
            initial: full,
            removed: all_keys,
            expected: BTreeMap::new(),
        },
    ]
}

#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let router = emulator();

    for (index, row) in removal_cases().into_iter().enumerate() {
        let resources = taggable_resources(&router, row.case, index, &row.initial).await;
        for (kind, arn) in resources {
            let case = format!("{} / {kind}", row.case);

            let (status, body) = untag_resource(&router, &arn, &row.removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: untagging an existing resource is HTTP 200, got {status} \
                 with {body}"
            );
            let stored = persisted_tags(&router, &case, &arn).await;
            assert_eq!(
                stored, row.expected,
                "case {case:?}: expected exactly the keys that were not removed ({} \
                 entries), got {stored:?}",
                row.expected.len()
            );

            // `UntagResource` is `@idempotent`: naming the same keys again removes
            // nothing further and answers the same.
            let (repeat_status, repeat_body) = untag_resource(&router, &arn, &row.removed).await;
            assert_eq!(
                repeat_status, 200,
                "case {case:?}: repeating the removal is HTTP 200, got {repeat_status} \
                 with {repeat_body}"
            );
            let after_repeat = persisted_tags(&router, &case, &arn).await;
            assert_eq!(
                after_repeat, row.expected,
                "case {case:?}: repeating the removal must change nothing, got \
                 {after_repeat:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_resources_tag_writes_are_never_anothers() {
    // The rule's other half: the write lands on the record the ARN names, so a cluster,
    // a second cluster and a snapshot of the first keep their own maps. Rows differ in
    // which resource is written to and with which keys.
    let router = emulator();

    let shared = map(&[("Project", "lionfish")]);
    let left = create_cluster(&router, "the left cluster", "isolated-left", &shared).await;
    let right = create_cluster(&router, "the right cluster", "isolated-right", &shared).await;
    let snapshot = create_snapshot(
        &router,
        "the snapshot of the left cluster",
        "isolated-snapshot",
        &left,
        &shared,
    )
    .await;

    // One write per resource, each naming keys the others must not acquire.
    let (status, body) = tag_resource(&router, &left, &map(&[("Side", "left")])).await;
    assert_eq!(status, 200, "tagging the left cluster: got {status} with {body}");
    let (status, body) = tag_resource(&router, &snapshot, &map(&[("Side", "snapshot")])).await;
    assert_eq!(status, 200, "tagging the snapshot: got {status} with {body}");
    // And one removal, which must not reach the other resources' copies of the key.
    let (status, body) = untag_resource(&router, &right, &keys(&["Project"])).await;
    assert_eq!(status, 200, "untagging the right cluster: got {status} with {body}");

    let rows = [
        (
            "the written cluster",
            &left,
            map(&[("Project", "lionfish"), ("Side", "left")]),
        ),
        ("the untagged cluster", &right, BTreeMap::new()),
        (
            "the written snapshot",
            &snapshot,
            map(&[("Project", "lionfish"), ("Side", "snapshot")]),
        ),
    ];
    for (case, arn, expected) in rows {
        let stored = persisted_tags(&router, case, arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected this resource's own tag map, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_write_changes_only_the_tags() {
    // Tagging is metadata: it moves no status and changes no other member of the record
    // it writes to. Rows span both taggable kinds and both tagging operations.
    let router = emulator();

    let initial = map(&[("Project", "lionfish")]);
    let cluster_arn = create_cluster(&router, "the cluster", "metadata-only", &initial).await;
    let snapshot_arn = create_snapshot(
        &router,
        "the snapshot",
        "metadata-only-snap",
        &cluster_arn,
        &initial,
    )
    .await;

    for (kind, arn, reader, member) in [
        (CLUSTER_TYPE, &cluster_arn, "/cluster/", "cluster"),
        (
            SNAPSHOT_TYPE,
            &snapshot_arn,
            "/cluster-snapshot/",
            "snapshot",
        ),
    ] {
        let path = format!("{reader}{}", encode(arn));
        let (status, before) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the resource must be readable, got {status} with {before}"
        );

        let (status, body) = tag_resource(&router, arn, &map(&[("Tier", "gold")])).await;
        assert_eq!(status, 200, "case {kind:?}: tagging: got {status} with {body}");
        let (status, body) = untag_resource(&router, arn, &keys(&["Project"])).await;
        assert_eq!(status, 200, "case {kind:?}: untagging: got {status} with {body}");

        let (status, after) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the resource must still be readable, got {status} with {after}"
        );
        // The projection carries no tags member, so the whole record must read back
        // exactly as it did before the writes.
        assert_eq!(
            after[member], before[member],
            "case {kind:?}: a tag write must change nothing else on the record, got \
             {after} after {before}"
        );
        let stored = persisted_tags(&router, kind, arn).await;
        assert_eq!(
            stored,
            map(&[("Tier", "gold")]),
            "case {kind:?}: expected the written key and not the removed one, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn writing_tags_on_an_absent_resource_is_a_not_found() {
    // Rows differ along which kind's resource segment the ARN spells, its region, its
    // account and whether it is an ARN at all: the reported type name follows the ARN's
    // own resource segment, and an identifier that spells no segment this service owns
    // addresses its primary taggable resource, the elastic cluster.
    let router = emulator();

    // A tagged cluster DOES exist, so each refusal is about the ARN the request named
    // rather than about an empty store.
    let present = map(&[("Project", "lionfish")]);
    let present_arn = create_cluster(&router, "the present cluster", "present-writes", &present).await;

    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a cluster ARN in the signing region",
            format!("arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/11111111-2222-3333-4444-555555555555"),
            CLUSTER_TYPE,
        ),
        (
            "a cluster ARN in another region",
            format!("arn:aws:docdb-elastic:us-west-2:{ACCOUNT}:cluster/00000000-0000-0000-0000-000000000000"),
            CLUSTER_TYPE,
        ),
        (
            "a snapshot ARN in the signing region",
            format!("arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster-snapshot/99999999-8888-7777-6666-555555555555"),
            SNAPSHOT_TYPE,
        ),
        (
            "a snapshot ARN in another account",
            format!("arn:aws:docdb-elastic:{REGION}:210987654321:cluster-snapshot/ABCDEF01-2345-6789-abcd-ef0123456789"),
            SNAPSHOT_TYPE,
        ),
        (
            "an identifier that is not an ARN at all",
            "not-an-arn-at-all".to_string(),
            CLUSTER_TYPE,
        ),
    ];

    for (row, arn, expected_type) in rows {
        let written = map(&[("Project", "lionfish")]);
        let attempts = [
            ("TagResource", tag_resource(&router, &arn, &written).await),
            (
                "UntagResource",
                untag_resource(&router, &arn, &keys(&["Project"])).await,
            ),
        ];
        for (operation, (status, body)) in attempts {
            let case = format!("{row} / {operation}");
            assert_eq!(
                status, 404,
                "case {case:?}: writing tags on an absent resource is HTTP 404, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(expected_type),
                "case {case:?}: resourceType must be {expected_type:?}, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: resourceId must be the ARN the request named, got {body}"
            );
        }

        // The refused write persisted nothing: the ARN still holds no resource.
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {row:?}: a refused write must persist nothing, got {status} with {body}"
        );
    }

    // The present resource is unaffected by every refusal above.
    let stored = persisted_tags(&router, "the present cluster", &present_arn).await;
    assert_eq!(
        stored, present,
        "the existing cluster must still carry its own tag map, got {stored:?}"
    );
}

#[tokio::test]
async fn a_write_past_the_published_maximum_number_of_tags_is_refused() {
    // The bound is the published maximum tag count; the rows are the maximum itself, one
    // past it, and a write at the maximum that adds no key.
    let router = emulator();

    let full: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    let arn = create_cluster(&router, "a cluster at the maximum", "tag-capacity", &full).await;

    // At the maximum, a write that only overwrites keys the resource already has changes
    // no count and is accepted.
    let overwrite = map(&[("key0", "rewritten")]);
    let (status, body) = tag_resource(&router, &arn, &overwrite).await;
    assert_eq!(
        status, 200,
        "case \"overwriting a key at the maximum\": expected HTTP 200, got {status} with {body}"
    );
    let mut expected = full.clone();
    expected.insert("key0".to_string(), "rewritten".to_string());
    let stored = persisted_tags(&router, "overwriting a key at the maximum", &arn).await;
    assert_eq!(
        stored, expected,
        "case \"overwriting a key at the maximum\": expected the rewritten value and no \
         new key, got {stored:?}"
    );

    // One key past the maximum is refused, and the resource keeps the map it had.
    let (status, body) = tag_resource(&router, &arn, &map(&[("oneTooMany", "x")])).await;
    assert_eq!(
        status, 400,
        "case \"one key past the maximum\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"one key past the maximum\": expected a ValidationException, got {body}"
    );
    let stored = persisted_tags(&router, "one key past the maximum", &arn).await;
    assert_eq!(
        stored, expected,
        "case \"one key past the maximum\": the refused write must persist nothing, got \
         {stored:?}"
    );
}
