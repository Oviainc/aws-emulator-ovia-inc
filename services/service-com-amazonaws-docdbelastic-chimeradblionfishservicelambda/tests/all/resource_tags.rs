// SPDX-License-Identifier: CC0-1.0

//! Reading a resource's tags back over HTTP, for every kind of resource this service
//! tags.
//!
//! The rule these suites assert: **`ListTagsForResource` reports exactly the tag map
//! persisted on the resource its `resourceArn` names** — every key and value as
//! stored, an empty map for a resource that carries no tags, one resource's tags never
//! another's — and, when this emulator holds no resource under that ARN, a 404
//! `ResourceNotFoundException` whose `resourceType` follows the ARN's own resource
//! segment.
//!
//! The rule generalises over the kind of resource, the SIZE of the tag map and the
//! FORM of its keys and values, so the table below spans both taggable kinds and maps
//! that differ along count (none, empty, one, several, the published maximum) and
//! along key/value form (every punctuation character `TagKey` admits, an empty value,
//! a key and a value at their published maximum length). The recorded case — a cluster
//! created with a couple of ordinary tags — is one row of it.

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
/// Source: the AWS-wide tag limit this service's state module pins as `MAX_TAGS`.
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

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does: a label is a single
/// path segment and an ARN carries both `:` and `/`, so neither may survive raw.
fn label(value: &str) -> String {
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

/// `GET /tags/{resourceArn}`: the status and body of a `ListTagsForResource` call.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// The `tags` member of a `ListTagsForResource` response, as a comparable map.
///
/// A JSON object has no order, so the comparison is map-to-map; `None` means the
/// member was absent (or not an object), which is a DIFFERENT answer from an empty
/// map and is reported as such by the assertions below.
fn reported_tags(body: &Value) -> Option<BTreeMap<String, String>> {
    let object = body.get("tags")?.as_object()?;
    Some(
        object
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
    )
}

/// One row of the table: a tag map to persist, labelled by what makes it differ.
struct TagCase {
    case: &'static str,
    tags: BTreeMap<String, String>,
    /// How the tag map travels on the create request: absent entirely, or as a (possibly
    /// empty) `tags` member.
    send_member: bool,
}

/// A map built from `(key, value)` pairs.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Tag maps differing along count and along the form of their keys and values.
fn tag_cases() -> Vec<TagCase> {
    // Every punctuation character the TagKey pattern admits, in one key.
    let punctuation_key: String = TAG_KEY_PUNCTUATION.iter().collect();
    // The published maxima, which an entry at the bound must survive.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    // The published maximum number of entries.
    let full_map: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();

    vec![
        TagCase {
            case: "no tags member at all",
            tags: BTreeMap::new(),
            send_member: false,
        },
        TagCase {
            case: "an explicitly empty tag map",
            tags: BTreeMap::new(),
            send_member: true,
        },
        TagCase {
            case: "a single tag",
            tags: map(&[("Project", "lionfish")]),
            send_member: true,
        },
        TagCase {
            case: "several ordinary tags",
            tags: map(&[
                ("Project", "lionfish"),
                ("Environment", "staging"),
                ("Owner", "platform"),
            ]),
            send_member: true,
        },
        TagCase {
            case: "a tag whose value is empty",
            tags: map(&[("Placeholder", ""), ("Project", "lionfish")]),
            send_member: true,
        },
        TagCase {
            case: "a key carrying every admitted punctuation character",
            tags: [
                (punctuation_key, "punctuated".to_string()),
                ("plain".to_string(), "value".to_string()),
            ]
            .into_iter()
            .collect(),
            send_member: true,
        },
        TagCase {
            case: "a key and a value at their published maximum length",
            tags: [
                (longest_key, longest_value),
                ("short".to_string(), "s".to_string()),
            ]
            .into_iter()
            .collect(),
            send_member: true,
        },
        TagCase {
            case: "the published maximum number of tags",
            tags: full_map,
            send_member: true,
        },
    ]
}

/// The `tags` member of a create request for `case`, if it travels at all.
fn tags_member(case: &TagCase) -> Option<Value> {
    if !case.send_member {
        return None;
    }
    let mut object = Map::new();
    for (key, value) in &case.tags {
        object.insert(key.clone(), Value::String(value.clone()));
    }
    Some(Value::Object(object))
}

/// A `CreateCluster` body for `name`, carrying `case`'s tag map when it travels.
fn create_cluster_body(name: &str, case: &TagCase) -> Value {
    let mut body = json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    });
    if let Some(tags) = tags_member(case) {
        body["tags"] = tags;
    }
    body
}

/// A `CreateClusterSnapshot` body for `name` on `cluster_arn`, carrying `case`'s tag
/// map when it travels.
fn create_snapshot_body(name: &str, cluster_arn: &str, case: &TagCase) -> Value {
    let mut body = json!({"clusterArn": cluster_arn, "snapshotName": name});
    if let Some(tags) = tags_member(case) {
        body["tags"] = tags;
    }
    body
}

/// Create a cluster and return its ARN.
async fn create_cluster(router: &MultiServiceRouter, name: &str, case: &TagCase) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/cluster",
        Some(create_cluster_body(name, case)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating cluster {name:?} must succeed, got {status} with {created}",
        case = case.case
    );
    created["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the created cluster carries its ARN", case.case))
        .to_string()
}

#[tokio::test]
async fn listed_tags_are_the_tags_the_resource_was_created_with() {
    let router = emulator();

    for (index, case) in tag_cases().into_iter().enumerate() {
        // Each row gets its own cluster and its own snapshot of that cluster, so the
        // two kinds are read back through the SAME rule over the same tag map.
        let cluster_name = format!("tagged-cluster-{index}");
        let cluster_arn = create_cluster(&router, &cluster_name, &case).await;

        let snapshot_name = format!("tagged-snapshot-{index}");
        let (status, snapshot) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(create_snapshot_body(&snapshot_name, &cluster_arn, &case)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating snapshot {snapshot_name:?} must succeed, got {status} \
             with {snapshot}",
            case = case.case
        );
        let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the created snapshot carries its ARN", case.case))
            .to_string();

        for (kind, arn) in [("Cluster", &cluster_arn), ("ClusterSnapshot", &snapshot_arn)] {
            let case_name = format!("{} / {kind}", case.case);
            let (status, body) = list_tags(&router, arn).await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: listing tags of an existing resource is HTTP 200, \
                 got {status} with {body}"
            );
            let reported = reported_tags(&body);
            assert_eq!(
                reported.as_ref(),
                Some(&case.tags),
                "case {case_name:?}: expected exactly the persisted tag map ({} entries), \
                 got {body}",
                case.tags.len()
            );
        }
    }
}

#[tokio::test]
async fn one_resources_tags_are_never_anothers() {
    // The rule's other half: the map is read from the record the ARN names, so two
    // resources of the same kind, and a cluster and its own snapshot, report their own
    // tags — the rows differ in which resource carries which map, including a resource
    // carrying none.
    let router = emulator();

    let cluster_tags = map(&[("Project", "left"), ("Tier", "gold")]);
    let snapshot_tags = map(&[("Project", "right")]);

    let tagged = TagCase {
        case: "the cluster's own tags",
        tags: cluster_tags.clone(),
        send_member: true,
    };
    let untagged = TagCase {
        case: "a cluster with no tags",
        tags: BTreeMap::new(),
        send_member: false,
    };

    let left_arn = create_cluster(&router, "distinct-left", &tagged).await;
    let right_arn = create_cluster(&router, "distinct-right", &untagged).await;

    let snapshot_case = TagCase {
        case: "the snapshot's own tags",
        tags: snapshot_tags.clone(),
        send_member: true,
    };
    let (status, snapshot) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(create_snapshot_body(
            "distinct-snapshot",
            &left_arn,
            &snapshot_case,
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "the snapshot must be created, got {status} with {snapshot}"
    );
    let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
        .as_str()
        .expect("the created snapshot carries its ARN")
        .to_string();

    let rows = [
        ("the tagged cluster", &left_arn, cluster_tags),
        ("the untagged cluster", &right_arn, BTreeMap::new()),
        (
            "the snapshot of the tagged cluster",
            &snapshot_arn,
            snapshot_tags,
        ),
    ];

    for (case, arn, expected) in rows {
        let (status, body) = list_tags(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing tags is HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            reported_tags(&body).as_ref(),
            Some(&expected),
            "case {case:?}: expected this resource's own tag map, got {body}"
        );
    }
}

#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    // `ListTagsForResource` is `@readonly`, so repeating it answers the same and leaves
    // the resources it read exactly as they were. Rows span both kinds.
    let router = emulator();

    let case = TagCase {
        case: "a tagged cluster",
        tags: map(&[("Project", "lionfish"), ("Tier", "gold")]),
        send_member: true,
    };
    let cluster_arn = create_cluster(&router, "stable-reads", &case).await;
    let (status, snapshot) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(create_snapshot_body("stable-reads-snap", &cluster_arn, &case)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the snapshot must be created, got {status} with {snapshot}"
    );
    let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
        .as_str()
        .expect("the created snapshot carries its ARN")
        .to_string();

    for (kind, arn, reader) in [
        ("Cluster", &cluster_arn, "/cluster/"),
        ("ClusterSnapshot", &snapshot_arn, "/cluster-snapshot/"),
    ] {
        let (_, first) = list_tags(&router, arn).await;
        let (_, second) = list_tags(&router, arn).await;
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {kind:?}: two reads in a row must report the same tags, got {first} \
             then {second}"
        );

        // And the resource itself is still readable, with the tags unchanged after it.
        let path = format!("{reader}{}", label(arn));
        let (status, fetched) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the resource must still be readable, got {status} with {fetched}"
        );
        let (_, third) = list_tags(&router, arn).await;
        assert_eq!(
            reported_tags(&third),
            reported_tags(&first),
            "case {kind:?}: the tags must be unchanged by reading, got {third}"
        );
    }
}

#[tokio::test]
async fn listing_tags_of_an_absent_resource_is_a_not_found() {
    // Rows differ along which kind's resource segment the ARN spells, its region, its
    // account and whether it is an ARN at all: the reported type name follows the ARN's
    // own resource segment, and an identifier that spells no segment this service owns
    // addresses its primary taggable resource, the elastic cluster.
    let router = emulator();

    // A tagged cluster DOES exist, so the suite proves the refusal is about the ARN the
    // request named rather than about an empty store.
    let present = TagCase {
        case: "a tagged cluster",
        tags: map(&[("Project", "lionfish")]),
        send_member: true,
    };
    let present_arn = create_cluster(&router, "present-cluster", &present).await;

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

    for (case, arn, expected_type) in rows {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent resource is HTTP 404, got {status} with {body}"
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

    // The present resource is unaffected by every refusal above.
    let (status, body) = list_tags(&router, &present_arn).await;
    assert_eq!(
        status, 200,
        "the existing cluster must still report its tags, got {status} with {body}"
    );
    assert_eq!(
        reported_tags(&body).as_ref(),
        Some(&present.tags),
        "the existing cluster must still report its own tag map, got {body}"
    );
}
