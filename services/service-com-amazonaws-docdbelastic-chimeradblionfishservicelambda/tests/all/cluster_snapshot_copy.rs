// SPDX-License-Identifier: CC0-1.0

//! How `CopyClusterSnapshot` mints a snapshot from another snapshot.
//!
//! The rule these suites assert: a copy is a NEW manual snapshot with its own name, ARN
//! and creation time, whose every other member is the one the SOURCE snapshot holds —
//! the cluster it was taken from, that cluster's creation time, the administrator user
//! name, the network identifiers and the configuration a restore needs — with
//! `kmsKeyId` taken from the request when it names one and from the source otherwise,
//! and tags that are the request's own plus, only when `copyTags` is true, the source's.
//! The copy is independent: deleting the source snapshot leaves it readable and
//! restorable.
//!
//! The rule generalises over which members a request carries, over the source snapshot
//! and over the region the request is signed for, so the table below spans both
//! `copyTags` spellings and its absent (documented-default) form, request tags that do
//! and do not collide with the source's, a named and an absent `kmsKeyId`, and a second
//! region.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The region the suites sign with unless a row names another.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `@required` members of `ClusterSnapshot`.
///
/// Source: `smithy_query(query="shape", shape="ClusterSnapshot")` — every member except
/// `snapshotType` is `@required`.
const REQUIRED_SNAPSHOT_MEMBERS: &[&str] = &[
    "subnetIds",
    "snapshotName",
    "snapshotArn",
    "snapshotCreationTime",
    "clusterArn",
    "clusterCreationTime",
    "status",
    "vpcSecurityGroupIds",
    "adminUserName",
    "kmsKeyId",
];

/// The members a copy inherits unchanged from the snapshot it was made from.
const INHERITED_MEMBERS: &[&str] = &[
    "clusterArn",
    "clusterCreationTime",
    "adminUserName",
    "subnetIds",
    "vpcSecurityGroupIds",
];

/// The `SnapshotType` a caller-made snapshot carries.
///
/// Source: `smithy_query(query="shape", shape="SnapshotType")` — `MANUAL` / `AUTOMATED`,
/// of which AWS reserves `AUTOMATED` for the snapshots it takes itself.
const MANUAL: &str = "MANUAL";

/// The `Status` a snapshot being copied carries before the clock settles it.
///
/// Source: `smithy_query(query="shape", shape="Status")` — `COPYING` is one of the
/// modelled values, and `ACTIVE` is the one a settled snapshot reports.
const COPYING: &str = "COPYING";
const ACTIVE: &str = "ACTIVE";

/// The inclusive length bounds of `CopyClusterSnapshotInput$targetSnapshotName`.
///
/// Source: `smithy_query(query="operation", operation="CopyClusterSnapshot")` —
/// `targetSnapshotName` carries `length(min: 1, max: 63)`.
const TARGET_NAME_LEN: (usize, usize) = (1, 63);

/// The most tags one resource carries, and the longest `kmsKeyId` this service stores
/// (`MAX_TAGS` / `MAX_KMS_KEY_ID_LEN` in `src/state.rs`).
const MAX_TAGS: usize = 50;
const MAX_KMS_KEY_ID_LEN: usize = 2048;

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

/// Percent-encode one `@httpLabel`, the way an AWS SDK does.
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

/// Send one request signed for `region` and return its status and parsed JSON body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{region}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{region}.amazonaws.com"))
        .header("authorization", authorization(region))
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

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, path, body).await
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A `CreateCluster` body whose network identifiers and KMS key are the row's own, so
/// an inherited member cannot pass by accident.
fn create_cluster_body(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": format!("admin{name}"),
        "adminUserPassword": "Secret123!",
        "shardCapacity": 4,
        "shardCount": 2,
        "subnetIds": [format!("subnet-{name}-a"), format!("subnet-{name}-b")],
        "vpcSecurityGroupIds": [format!("sg-{name}")],
        "kmsKeyId": format!("alias/{name}-key"),
    })
}

/// Create a cluster in `region` that must succeed, returning its `Cluster`.
async fn create_cluster(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, response) =
        call_in(router, region, "POST", "/cluster", Some(create_cluster_body(name))).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture cluster must be created, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Snapshot `cluster_arn` under `name` with `tags`, returning the new `ClusterSnapshot`.
async fn create_snapshot(
    router: &MultiServiceRouter,
    region: &str,
    cluster_arn: &str,
    name: &str,
    tags: Value,
) -> Value {
    let (status, response) = call_in(
        router,
        region,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": cluster_arn, "snapshotName": name, "tags": tags})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture snapshot must be created, got {status} with {response}"
    );
    response["snapshot"].clone()
}

/// `GET /cluster-snapshot/{arn}`, which must answer 200, returning the `ClusterSnapshot`.
async fn get_snapshot(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) =
        call(router, "GET", &format!("/cluster-snapshot/{}", label(arn)), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the snapshot {arn:?} must be readable, got {status} with {response}"
    );
    response["snapshot"].clone()
}

/// The tags `ListTagsForResource` reports for `arn`, as a sorted map.
async fn tags_of(router: &MultiServiceRouter, case: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, response) = call(router, "GET", &format!("/tags/{}", label(arn)), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the tags of {arn:?} must be readable, got {status} with {response}"
    );
    response["tags"]
        .as_object()
        .map(|map| {
            map.iter()
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

/// One row of the copy table.
struct Row {
    case: &'static str,
    region: &'static str,
    /// The request's `copyTags`, or `None` to leave the member out and take its
    /// documented default (false).
    copy_tags: Option<bool>,
    /// The request's `tags`, or `None` to leave the member out.
    tags: Option<&'static [(&'static str, &'static str)]>,
    /// The request's `kmsKeyId`, or `None` to leave the member out.
    kms_key_id: Option<&'static str>,
}

#[tokio::test]
async fn copy_cluster_snapshot_mints_an_independent_manual_snapshot_from_the_source() {
    // The source snapshot's own tags, which only a `copyTags: true` row inherits. The
    // `shared` key is also sent by one row's request tags, so a collision is covered.
    let source_tags: &[(&str, &str)] = &[("source", "yes"), ("shared", "from-source")];

    let rows = [
        Row {
            case: "no copyTags and no tags: the copy carries none",
            region: REGION,
            copy_tags: None,
            tags: None,
            kms_key_id: None,
        },
        Row {
            case: "copyTags false with request tags: only the request's",
            region: REGION,
            copy_tags: Some(false),
            tags: Some(&[("owner", "team-a")]),
            kms_key_id: None,
        },
        Row {
            case: "copyTags true with no request tags: the source's",
            region: REGION,
            copy_tags: Some(true),
            tags: None,
            kms_key_id: None,
        },
        Row {
            case: "copyTags true with a request tag colliding with a source tag",
            region: REGION,
            copy_tags: Some(true),
            tags: Some(&[("shared", "from-request"), ("owner", "team-b")]),
            kms_key_id: None,
        },
        Row {
            case: "a named kmsKeyId encrypts the copy with that key",
            region: REGION,
            copy_tags: Some(true),
            tags: None,
            kms_key_id: Some("arn:aws:kms:eu-west-2:000000000000:key/copy-target-key"),
        },
        Row {
            case: "a request signed for another region mints the copy there",
            region: "ap-south-1",
            copy_tags: None,
            tags: Some(&[("region", "ap-south-1")]),
            kms_key_id: None,
        },
    ];

    let router = emulator();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let cluster = create_cluster(&router, row.region, &format!("copysrc{index}")).await;
        let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();
        let source = create_snapshot(
            &router,
            row.region,
            &cluster_arn,
            &format!("source-snap-{index}"),
            json!(source_tags.iter().copied().collect::<BTreeMap<_, _>>()),
        )
        .await;
        let source_arn = source["snapshotArn"].as_str().expect("snapshotArn").to_string();

        let target_name = format!("copy-snap-{index}");
        let mut body = json!({"targetSnapshotName": target_name});
        if let Some(copy_tags) = row.copy_tags {
            body["copyTags"] = json!(copy_tags);
        }
        if let Some(tags) = row.tags {
            body["tags"] = json!(tags.iter().copied().collect::<BTreeMap<_, _>>());
        }
        if let Some(key) = row.kms_key_id {
            body["kmsKeyId"] = json!(key);
        }
        let (status, response) = call_in(
            &router,
            row.region,
            "POST",
            &format!("/cluster-snapshot/{}/copy", label(&source_arn)),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {body}, got {status} with {response}"
        );
        let copy = response["snapshot"].clone();

        // Every `@required` member of the modelled shape is populated.
        for member in REQUIRED_SNAPSHOT_MEMBERS {
            assert!(
                copy.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: ClusterSnapshot member `{member}` missing in {copy}"
            );
        }

        // The copy is a new resource: its own name and ARN, minted in the region the
        // request was signed for.
        assert_eq!(
            copy["snapshotName"].as_str(),
            Some(target_name.as_str()),
            "case {case:?}: expected snapshotName {target_name:?}, got {copy}"
        );
        let copy_arn = copy["snapshotArn"].as_str().expect("snapshotArn").to_string();
        assert_ne!(
            copy_arn, source_arn,
            "case {case:?}: the copy must carry its own ARN, got {copy_arn:?}"
        );
        let expected_prefix = format!(
            "arn:aws:docdb-elastic:{}:{ACCOUNT}:cluster-snapshot/",
            row.region
        );
        assert!(
            copy_arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN starting {expected_prefix:?}, got {copy_arn:?}"
        );
        // A caller-made snapshot is MANUAL, and a copy in flight reports COPYING.
        assert_eq!(
            copy["snapshotType"].as_str(),
            Some(MANUAL),
            "case {case:?}: expected snapshotType {MANUAL}, got {copy}"
        );
        assert_eq!(
            copy["status"].as_str(),
            Some(COPYING),
            "case {case:?}: expected status {COPYING} while the copy is in flight, got {copy}"
        );
        assert_ne!(
            copy["snapshotCreationTime"].as_str(),
            None,
            "case {case:?}: the copy must carry its own creation time, got {copy}"
        );

        // Everything else comes from the source snapshot.
        for member in INHERITED_MEMBERS {
            assert_eq!(
                copy.get(member),
                source.get(member),
                "case {case:?}: expected `{member}` {:?} inherited from the source, got {:?}",
                source.get(member),
                copy.get(member)
            );
        }
        let expected_key = row
            .kms_key_id
            .map(str::to_string)
            .unwrap_or_else(|| source["kmsKeyId"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            copy["kmsKeyId"].as_str(),
            Some(expected_key.as_str()),
            "case {case:?}: expected kmsKeyId {expected_key:?}, got {copy}"
        );

        // Tags: the request's own, plus the source's only when `copyTags` is true.
        let mut expected_tags: BTreeMap<String, String> = BTreeMap::new();
        if row.copy_tags == Some(true) {
            expected_tags.extend(
                source_tags
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string())),
            );
        }
        if let Some(tags) = row.tags {
            expected_tags.extend(
                tags.iter()
                    .map(|(key, value)| (key.to_string(), value.to_string())),
            );
        }
        let reported_tags = tags_of(&router, case, &copy_arn).await;
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {reported_tags:?}"
        );
        // The source snapshot keeps its own tags: a copy reads it, it never edits it.
        let source_reported = tags_of(&router, case, &source_arn).await;
        let source_expected: BTreeMap<String, String> = source_tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            source_reported, source_expected,
            "case {case:?}: the source snapshot's tags must be untouched, got {source_reported:?}"
        );

        // Round trip: the reader reports the stored copy, and the status it reports is
        // either the one in flight or the ACTIVE the clock settles it to.
        let read = get_snapshot(&router, case, &copy_arn).await;
        for member in REQUIRED_SNAPSHOT_MEMBERS {
            if *member == "status" {
                continue;
            }
            assert_eq!(
                read.get(member),
                copy.get(member),
                "case {case:?}: the reader disagrees with the copy on `{member}`: {read} vs {copy}"
            );
        }
        let read_status = read["status"].as_str().unwrap_or_default();
        assert!(
            read_status == COPYING || read_status == ACTIVE,
            "case {case:?}: expected the copy to report {COPYING} or {ACTIVE}, got {read_status:?}"
        );

        // Independence: the copy outlives the snapshot it was made from, and a restore
        // from it rebuilds the source cluster's configuration.
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/cluster-snapshot/{}", label(&source_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the source snapshot must be deletable, got {status} with {deleted}"
        );
        let after = get_snapshot(&router, case, &copy_arn).await;
        assert_eq!(
            after["snapshotArn"].as_str(),
            Some(copy_arn.as_str()),
            "case {case:?}: the copy must survive the source's deletion, got {after}"
        );
        let (status, restored) = call(
            &router,
            "POST",
            &format!("/cluster-snapshot/{}/restore", label(&copy_arn)),
            Some(json!({"clusterName": format!("restored{index}")})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a restore from the copy must succeed, got {status} with {restored}"
        );
        for member in ["shardCapacity", "shardCount", "adminUserName"] {
            assert_eq!(
                restored["cluster"].get(member),
                cluster.get(member),
                "case {case:?}: the restored cluster must keep the source's `{member}`: {:?} vs \
                 {:?}",
                cluster.get(member),
                restored["cluster"].get(member)
            );
        }
    }
}

#[tokio::test]
async fn copy_cluster_snapshot_refuses_an_unknown_source_a_taken_name_and_unbounded_values() {
    let router = emulator();
    let cluster = create_cluster(&router, REGION, "refusalsrc").await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();
    let source = create_snapshot(&router, REGION, &cluster_arn, "refusal-source", json!({})).await;
    let source_arn = source["snapshotArn"].as_str().expect("snapshotArn").to_string();
    let copy_path = format!("/cluster-snapshot/{}/copy", label(&source_arn));

    // An ARN this store never minted is 404, whatever else the request carries.
    let unknown = format!(
        "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster-snapshot/66666666-7777-8888-9999-000000000000"
    );
    for (case, name) in [
        ("a target name nobody has taken", "free-target"),
        ("a target name already taken", "refusal-source"),
    ] {
        let (status, response) = call(
            &router,
            "POST",
            &format!("/cluster-snapshot/{}/copy", label(&unknown)),
            Some(json!({"targetSnapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown source snapshot must be 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {response}"
        );
    }

    // A target name another snapshot already holds conflicts, and nothing is minted.
    for (case, name) in [
        ("the source snapshot's own name", "refusal-source"),
        ("a name taken by an earlier copy", "first-copy"),
    ] {
        if name == "first-copy" {
            let (status, response) = call(
                &router,
                "POST",
                &copy_path,
                Some(json!({"targetSnapshotName": name})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first copy must succeed, got {status} with {response}"
            );
        }
        let (status, response) = call(
            &router,
            "POST",
            &copy_path,
            Some(json!({"targetSnapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a taken target name must be 409, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {response}"
        );
    }

    // `targetSnapshotName` is modelled length(1, 63): both published bounds pass and one
    // step past each is refused. Caller-sized `tags` and `kmsKeyId` are bounded too.
    let (min, max) = TARGET_NAME_LEN;
    let too_many_tags: Value = Value::Object(
        (0..=MAX_TAGS)
            .map(|i| (format!("k{i}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>(),
    );
    let at_the_tag_bound: Value = Value::Object(
        (0..MAX_TAGS)
            .map(|i| (format!("k{i}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>(),
    );
    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "the shortest allowed target name",
            json!({"targetSnapshotName": "a".repeat(min)}),
            200,
        ),
        (
            "a target name at the modelled maximum",
            json!({"targetSnapshotName": "b".repeat(max)}),
            200,
        ),
        (
            "a target name one character past the maximum",
            json!({"targetSnapshotName": "c".repeat(max + 1)}),
            400,
        ),
        (
            "an empty target name",
            json!({"targetSnapshotName": ""}),
            400,
        ),
        (
            "tags at the bound",
            json!({"targetSnapshotName": "tags-at-bound", "tags": at_the_tag_bound}),
            200,
        ),
        (
            "tags one past the bound",
            json!({"targetSnapshotName": "tags-past-bound", "tags": too_many_tags}),
            400,
        ),
        (
            "a tag value one character too long",
            json!({"targetSnapshotName": "tag-value", "tags": {"k": "v".repeat(257)}}),
            400,
        ),
        (
            "a kmsKeyId at the bound",
            json!({"targetSnapshotName": "kms-at-bound", "kmsKeyId": "k".repeat(MAX_KMS_KEY_ID_LEN)}),
            200,
        ),
        (
            "a kmsKeyId one character past the bound",
            json!({
                "targetSnapshotName": "kms-past-bound",
                "kmsKeyId": "k".repeat(MAX_KMS_KEY_ID_LEN + 1)
            }),
            400,
        ),
    ];
    for (case, body, expected) in cases {
        let (status, response) = call(&router, "POST", &copy_path, Some(body.clone())).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for {body}, got {status} with {response}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response}"
            );
        }
    }

    // A copy that was refused left nothing behind: only the source and the copies that
    // answered 200 are stored.
    let (status, listed) = call(&router, "GET", "/cluster-snapshots", None).await;
    assert_eq!(
        status, 200,
        "case listing: the snapshots must be listable, got {status} with {listed}"
    );
    let mut names: Vec<String> = listed["snapshots"]
        .as_array()
        .expect("snapshots")
        .iter()
        .map(|item| item["snapshotName"].as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    let mut expected_names = vec![
        "refusal-source".to_string(),
        "first-copy".to_string(),
        "a".repeat(TARGET_NAME_LEN.0),
        "b".repeat(TARGET_NAME_LEN.1),
        "tags-at-bound".to_string(),
        "kms-at-bound".to_string(),
    ];
    expected_names.sort();
    assert_eq!(
        names, expected_names,
        "case listing: expected exactly {expected_names:?}, got {names:?}"
    );
}
