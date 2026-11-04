// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of a resource identifier it holds nothing under.
//!
//! The rule these suites assert: a request naming an identifier this store holds no
//! resource under is refused 404 `ResourceNotFoundException` whose `message` is the
//! FIXED sentence of the resource the operation ADDRESSED — the class's own sentence
//! where the operation was asked for a resource of one named class, and the generic
//! resource sentence where it addresses any class by bare ARN — the whole message being
//! fixed text, with no part taken from the request, and the identifier the request named
//! being reported in `resourceId` alone.
//!
//! The rule generalises over the operation, over the resource class and over the form
//! of the identifier, so the table below spans every not-found path of the implemented
//! lifecycle and, for each, identifiers differing in region, account, which ARN
//! resource segment they spell, their length and whether they are an ARN at all: the
//! sentence must be the same for all of them, which is exactly what an interpolated
//! message is not. The ARN-addressed tagging surface has its own suite
//! (`tagging_not_found_sentence`), which spans all three of its operations.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The sentence this service answers when a CLUSTER cannot be found.
///
/// Recorded AWS behavior for a `clusterArn` no cluster carries: the entire message,
/// with no variable part.
const CLUSTER_SENTENCE: &str = "Unable to find the cluster";

/// The same sentence for the other resource class this service owns, the cluster
/// snapshot: one fixed sentence per class, naming the class and nothing else.
const SNAPSHOT_SENTENCE: &str = "Unable to find the snapshot";

/// The sentence an operation that addresses ANY class by bare ARN answers instead,
/// naming no class at all.
///
/// Recorded AWS behavior for the tag operations, whose only identifier is an `Arn`.
const TAGGED_RESOURCE_SENTENCE: &str = "Resource was not found";

/// A SigV4 `Authorization` header for this service in [`REGION`].
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/aws4_request, \
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

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does.
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
        .header("authorization", authorization())
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

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A `CreateCluster` body with every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    })
}

/// One identifier a lookup can name, labelled by what makes it differ from the others.
struct Identifier {
    case: &'static str,
    value: String,
}

/// Identifiers differing in region, account, ARN resource segment, length and whether
/// they are an ARN at all. None of them can be found, whatever the operation.
///
/// `segment` is the ARN resource segment of the class the row is built for; each table
/// also carries a row spelling the OTHER class's segment, because the sentence follows
/// the resource the OPERATION addresses and not the text of the identifier.
fn identifiers(segment: &str, other_segment: &str) -> Vec<Identifier> {
    vec![
        Identifier {
            case: "an ARN of this class in the signing region",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{segment}/11111111-2222-3333-4444-555555555555"
            ),
        },
        Identifier {
            case: "an ARN of this class in another region",
            value: format!(
                "arn:aws:docdb-elastic:us-west-2:{ACCOUNT}:{segment}/00000000-0000-0000-0000-000000000000"
            ),
        },
        Identifier {
            case: "an ARN of this class in another account",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:210987654321:{segment}/ABCDEF01-2345-6789-abcd-ef0123456789"
            ),
        },
        Identifier {
            case: "an ARN spelling the other class's resource segment",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{other_segment}/99999999-8888-7777-6666-555555555555"
            ),
        },
        Identifier {
            case: "an ARN whose resource id is a long word",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{segment}/{}",
                "z".repeat(120)
            ),
        },
        Identifier {
            case: "an identifier that is not an ARN at all",
            value: "not-an-arn-at-all".to_string(),
        },
    ]
}

/// One not-found path: the request to make for a given identifier, and the sentence the
/// refusal must carry for it.
struct Lookup {
    operation: &'static str,
    method: &'static str,
    /// The path the identifier is addressed through.
    path: fn(&str) -> String,
    /// The request body, built from the identifier when the identifier travels in it.
    body: fn(&str) -> Option<Value>,
    /// The fixed sentence this path must answer, whatever identifier it was handed.
    sentence: &'static str,
    /// The ARN resource segment of the class looked up, and of the other class.
    segments: (&'static str, &'static str),
}

/// Every lookup of the implemented lifecycle that can answer "no such resource".
fn lookups() -> Vec<Lookup> {
    vec![
        Lookup {
            operation: "GetCluster",
            method: "GET",
            path: |arn| format!("/cluster/{}", label(arn)),
            body: |_| None,
            sentence: CLUSTER_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "UpdateCluster",
            method: "PUT",
            path: |arn| format!("/cluster/{}", label(arn)),
            body: |_| Some(json!({"shardCount": 2})),
            sentence: CLUSTER_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "DeleteCluster",
            method: "DELETE",
            path: |arn| format!("/cluster/{}", label(arn)),
            body: |_| None,
            sentence: CLUSTER_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            // The missing resource travels in the BODY here, and it is the cluster
            // being snapshotted — so the refusal is the CLUSTER sentence even though
            // the route is the snapshot collection.
            operation: "CreateClusterSnapshot",
            method: "POST",
            path: |_| "/cluster-snapshot".to_string(),
            body: |arn| Some(json!({"clusterArn": arn, "snapshotName": "probe-snapshot"})),
            sentence: CLUSTER_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "GetPendingMaintenanceAction",
            method: "GET",
            path: |arn| format!("/pending-action/{}", label(arn)),
            body: |_| None,
            sentence: CLUSTER_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "ApplyPendingMaintenanceAction",
            method: "POST",
            path: |_| "/pending-action".to_string(),
            body: |arn| {
                Some(json!({
                    "resourceArn": arn,
                    "applyAction": "ENGINE_UPDATE",
                    "optInType": "IMMEDIATE",
                }))
            },
            sentence: CLUSTER_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "GetClusterSnapshot",
            method: "GET",
            path: |arn| format!("/cluster-snapshot/{}", label(arn)),
            body: |_| None,
            sentence: SNAPSHOT_SENTENCE,
            segments: ("cluster-snapshot", "cluster"),
        },
        Lookup {
            operation: "DeleteClusterSnapshot",
            method: "DELETE",
            path: |arn| format!("/cluster-snapshot/{}", label(arn)),
            body: |_| None,
            sentence: SNAPSHOT_SENTENCE,
            segments: ("cluster-snapshot", "cluster"),
        },
        Lookup {
            operation: "RestoreClusterFromSnapshot",
            method: "POST",
            path: |arn| format!("/cluster-snapshot/{}/restore", label(arn)),
            body: |_| Some(json!({"clusterName": "restored"})),
            sentence: SNAPSHOT_SENTENCE,
            segments: ("cluster-snapshot", "cluster"),
        },
        Lookup {
            // A tagging call is asked for no resource class of its own — its only
            // identifier is a bare ARN of ANY class this service owns — so it words the
            // miss generically, whichever segment the ARN spells.
            operation: "ListTagsForResource",
            method: "GET",
            path: |arn| format!("/tags/{}", label(arn)),
            body: |_| None,
            sentence: TAGGED_RESOURCE_SENTENCE,
            segments: ("cluster", "cluster-snapshot"),
        },
    ]
}

#[tokio::test]
async fn a_not_found_refusal_states_the_fixed_sentence_of_its_resource_class() {
    let router = emulator();
    // A cluster and a snapshot DO exist, so every row below refuses a resource that is
    // genuinely absent from a non-empty store.
    let (status, created) = call(&router, "POST", "/cluster", Some(minimal_create("present"))).await;
    assert_eq!(
        status, 200,
        "the fixture cluster must be created, got {status} with {created}"
    );
    let cluster_arn = created["cluster"]["clusterArn"]
        .as_str()
        .expect("the created cluster carries its ARN")
        .to_string();
    let (status, snapshot) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": cluster_arn, "snapshotName": "present-snapshot"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture snapshot must be created, got {status} with {snapshot}"
    );

    for lookup in lookups() {
        let (segment, other_segment) = lookup.segments;
        for identifier in identifiers(segment, other_segment) {
            let case = format!("{} / {}", lookup.operation, identifier.case);
            let path = (lookup.path)(&identifier.value);
            let body = (lookup.body)(&identifier.value);
            let (status, response) = call(&router, lookup.method, &path, body).await;

            assert_eq!(
                status, 404,
                "case {case:?}: an absent resource is HTTP 404, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {response}"
            );

            let message = response["message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry `message`, got {response}"))
                .to_string();
            let expected = lookup.sentence;
            assert_eq!(
                message, expected,
                "case {case:?}: the refusal must be the fixed sentence {expected:?}, got \
                 {message:?}"
            );
            // Nothing the request carried may appear in the message: not the whole
            // identifier and not any field of it.
            for fragment in [
                identifier.value.as_str(),
                REGION,
                ACCOUNT,
                "arn:aws:docdb-elastic",
            ] {
                assert!(
                    !message.contains(fragment),
                    "case {case:?}: the message must carry no part of the request, found \
                     {fragment:?} in {message:?}"
                );
            }
            assert_eq!(
                response["resourceId"].as_str(),
                Some(identifier.value.as_str()),
                "case {case:?}: the identifier belongs in resourceId, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn one_sentence_serves_every_identifier_of_a_class() {
    // The defect this pins: a message built from the request varies per request, where
    // AWS's is constant. So the sentence is collected over every identifier of a class
    // and must come back as ONE distinct value per class, and the two classes must not
    // share it.
    let router = emulator();
    let mut cluster_messages = std::collections::BTreeSet::new();
    let mut snapshot_messages = std::collections::BTreeSet::new();

    for identifier in identifiers("cluster", "cluster-snapshot") {
        let path = format!("/cluster/{}", label(&identifier.value));
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {:?}: an absent cluster is HTTP 404, got {status} with {response}",
            identifier.case
        );
        cluster_messages.insert(response["message"].as_str().unwrap_or_default().to_string());
    }
    for identifier in identifiers("cluster-snapshot", "cluster") {
        let path = format!("/cluster-snapshot/{}", label(&identifier.value));
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {:?}: an absent snapshot is HTTP 404, got {status} with {response}",
            identifier.case
        );
        snapshot_messages.insert(response["message"].as_str().unwrap_or_default().to_string());
    }

    assert_eq!(
        cluster_messages,
        [CLUSTER_SENTENCE.to_string()].into_iter().collect(),
        "case cluster: every identifier must share one fixed sentence, got \
         {cluster_messages:?}"
    );
    assert_eq!(
        snapshot_messages,
        [SNAPSHOT_SENTENCE.to_string()].into_iter().collect(),
        "case snapshot: every identifier must share one fixed sentence, got \
         {snapshot_messages:?}"
    );
    assert_ne!(
        cluster_messages, snapshot_messages,
        "case both classes: each resource class states its own sentence, got \
         {cluster_messages:?}"
    );
}

#[tokio::test]
async fn a_refused_lookup_leaves_both_collections_answering() {
    // The positive controls of the recorded checks: a refusal is the whole effect of
    // the call — the listings still answer, and nothing was created by the attempt.
    let router = emulator();
    let absent = format!(
        "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/00000000-0000-0000-0000-000000000000"
    );

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "GetCluster on an absent cluster",
            "GET",
            format!("/cluster/{}", label(&absent)),
            None,
        ),
        (
            "CreateClusterSnapshot naming an absent cluster",
            "POST",
            "/cluster-snapshot".to_string(),
            Some(json!({"clusterArn": absent, "snapshotName": "orphan-snapshot"})),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(CLUSTER_SENTENCE),
            "case {case:?}: expected the fixed cluster sentence, got {response}"
        );

        for (collection, listing_path, member) in [
            ("ListClusters", "/clusters", "clusters"),
            ("ListClusterSnapshots", "/cluster-snapshots", "snapshots"),
        ] {
            let (status, listing) = call(&router, "GET", listing_path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {collection} must still answer, got {status} with {listing}"
            );
            let items = listing[member].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: {collection} must carry `{member}`, got {listing}")
            });
            assert!(
                items.is_empty(),
                "case {case:?}: the refusal must have created nothing, {collection} \
                 reported {items:?}"
            );
        }
    }
}
