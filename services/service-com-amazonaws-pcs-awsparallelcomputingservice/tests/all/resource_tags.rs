// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for PCS resource tagging — `TagResource`, `ListTagsForResource` and
//! `UntagResource` — driven over the awsJson1_0 wire (`POST /` with
//! `X-Amz-Target: AWSParallelComputingService.<Operation>`) through the same router the
//! emulator serves from, so these exercise routing, deserialization, constraint
//! validation, the handler and serialization.
//!
//! The rules under test:
//!
//! * a tag write merges into the tag set of the resource its `resourceArn` names — a new
//!   key is added, a key the resource already carries is edited — and a tag read answers
//!   that stored set, for every taggable PCS resource kind (cluster, compute node group,
//!   queue);
//! * removing a tag drops only the named keys and is idempotent in the key;
//! * a resource holds at most `MAX_TAGS_PER_RESOURCE` tags, and a write that would leave
//!   it with more is refused whole with the modelled `ServiceQuotaExceededException`;
//! * an ARN no stored resource carries is refused with `ResourceNotFoundException` on all
//!   three operations.
//!
//! Each test builds its own engine, so the state one test writes is invisible to the
//! others.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{
    MAX_TAGS_PER_RESOURCE, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to `pcs` in `us-east-1`. The router resolves a
/// request's service from the credential scope, and the handlers read the region out of
/// it to build an ARN. The signature is never checked: request authentication is off.
const SIGNED_FOR_PCS: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/pcs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region [`SIGNED_FOR_PCS`] signs for, which every ARN in these tests carries.
const REGION: &str = "us-east-1";

/// An emulator with only PCS mounted, on its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pcs_awsparallelcomputingservice::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AWSParallelComputingService.{operation}"),
        )
        .header("authorization", SIGNED_FOR_PCS)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body().clone();
    let parsed = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered {status} with a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Wait out the span the service stores for a lifecycle transition the clock owes, so the
/// next call sees it materialised. The service never sleeps for it — this is the CALLER
/// waiting, as a caller of the real service would.
async fn wait_past_the_owed_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(250)).await;
}

/// The three taggable PCS resources, each ACTIVE and carrying the tags its create asked
/// for: the cluster, a compute node group inside it and a queue inside it, by ARN.
struct Tagged {
    cluster_arn: String,
    compute_node_group_arn: String,
    queue_arn: String,
    cluster_id: String,
}

/// Create a cluster, a compute node group and a queue, each with `tags` set on the
/// create, and wait until the clock has made them all ACTIVE.
async fn tagged_resources(router: &MultiServiceRouter, tags: &Value) -> Tagged {
    let (status, cluster) = call(
        router,
        "CreateCluster",
        &json!({
            "clusterName": "tag-cluster",
            "scheduler": {"type": "SLURM", "version": "24.11"},
            "size": "SMALL",
            "networking": {"subnetIds": ["subnet-0123456789abcdef0"]},
            "tags": tags,
        }),
    )
    .await;
    assert_eq!(status, 200, "case CreateCluster: got {cluster}");
    let cluster_id = string_at(&cluster, &["cluster", "id"]);
    let cluster_arn = string_at(&cluster, &["cluster", "arn"]);
    wait_past_the_owed_transition().await;

    let (status, group) = call(
        router,
        "CreateComputeNodeGroup",
        &json!({
            "clusterIdentifier": &cluster_id,
            "computeNodeGroupName": "tag-cng",
            "subnetIds": ["subnet-0123456789abcdef0"],
            "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
            "iamInstanceProfileArn": format!(
                "arn:aws:iam::{}:instance-profile/AWSPCS-demo",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
            "instanceConfigs": [{"instanceType": "c6i.large"}],
            "tags": tags,
        }),
    )
    .await;
    assert_eq!(status, 200, "case CreateComputeNodeGroup: got {group}");
    let compute_node_group_arn = string_at(&group, &["computeNodeGroup", "arn"]);

    let (status, queue) = call(
        router,
        "CreateQueue",
        &json!({
            "clusterIdentifier": &cluster_id,
            "queueName": "tag-queue",
            "tags": tags,
        }),
    )
    .await;
    assert_eq!(status, 200, "case CreateQueue: got {queue}");
    let queue_arn = string_at(&queue, &["queue", "arn"]);
    wait_past_the_owed_transition().await;

    Tagged {
        cluster_arn,
        compute_node_group_arn,
        queue_arn,
        cluster_id,
    }
}

/// A string member of a response body, by path.
fn string_at(body: &Value, path: &[&str]) -> String {
    let mut cursor = body;
    for step in path {
        cursor = &cursor[step];
    }
    cursor
        .as_str()
        .unwrap_or_else(|| panic!("{path:?} must be a string in {body}"))
        .to_string()
}

/// The tags `ListTagsForResource` reports for `arn`, as a sorted map so a comparison does
/// not depend on map ordering.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
    assert_eq!(status, 200, "case ListTagsForResource {arn}: got {body}");
    body["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("case ListTagsForResource {arn}: no tags map in {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `count` distinct tags, keyed by `prefix`.
fn bulk_tags(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("{prefix}-{index:03}"), format!("value-{index:03}")))
        .collect()
}

#[tokio::test]
async fn a_tag_write_merges_into_the_resource_it_names_and_reads_back_whole() {
    let router = emulator();
    let created = tagged_resources(&router, &json!({"created": "by-the-create-call"})).await;

    // Rows: one tag write per row, differing along what the rule generalises over — the
    // number of entries, a brand-new key versus an edit of a key already held, and the
    // forms a `TagKey`/`TagValue` can take (`TagKey` is modelled @length(1..128) with no
    // pattern, `TagValue` @length(0..256), so punctuation, non-ASCII text, the length
    // bounds and an empty value are all in the class).
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let cases: Vec<(&str, Value)> = vec![
        ("one brand-new key", json!({"env": "prod"})),
        (
            "several keys at once",
            json!({"team": "hpc", "cost-center": "4711"}),
        ),
        (
            "an edit of a key already held",
            json!({"created": "edited-by-the-tag-call"}),
        ),
        ("an empty value", json!({"blank": ""})),
        (
            "punctuation and non-ASCII in the key",
            json!({"aws:pcs/tag.key_1": "値"}),
        ),
        (
            "the key and value length bounds",
            json!({long_key.clone(): long_value.clone()}),
        ),
        (
            "the same write repeated",
            json!({"created": "edited-by-the-tag-call"}),
        ),
    ];

    // Every resource kind carries tags the same way, so the table runs against all three.
    for (kind, arn) in [
        ("cluster", &created.cluster_arn),
        ("compute node group", &created.compute_node_group_arn),
        ("queue", &created.queue_arn),
    ] {
        let mut expected: BTreeMap<String, String> =
            [("created".to_string(), "by-the-create-call".to_string())]
                .into_iter()
                .collect();
        let from_create = listed_tags(&router, arn).await;
        assert_eq!(
            from_create, expected,
            "case {kind:?} tags set on the create: expected {expected:?}, got {from_create:?}"
        );

        for (case, tags) in &cases {
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({"resourceArn": arn, "tags": tags}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {kind:?}/{case:?}: expected HTTP 200, got {status} with {body:?}"
            );
            for (key, value) in tags.as_object().expect("the row is a tag map") {
                expected.insert(
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                );
            }
            let got = listed_tags(&router, arn).await;
            assert_eq!(
                got, expected,
                "case {kind:?}/{case:?}: expected {expected:?}, got {got:?}"
            );
        }

        // Removing tags drops exactly the keys named, and a key the resource does not
        // carry is nothing to remove — so the call still succeeds.
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({
                "resourceArn": arn,
                "tagKeys": ["env", "blank", "never-set", long_key.clone()],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {kind:?} untag including a key never set: expected HTTP 200, got {status} \
             with {body:?}"
        );
        for key in ["env", "blank", "never-set", long_key.as_str()] {
            expected.remove(key);
        }
        let got = listed_tags(&router, arn).await;
        assert_eq!(
            got, expected,
            "case {kind:?} after the untag: expected {expected:?}, got {got:?}"
        );
    }

    // Tags belong to the resource their ARN names: the writes above left each of the
    // three with its own set, and a tag written to one is never visible on another.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": &created.cluster_arn, "tags": {"only-on": "the-cluster"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a tag written to the cluster alone: expected HTTP 200, got {status} with {body:?}"
    );
    for (kind, arn, expected_present) in [
        ("cluster", &created.cluster_arn, true),
        ("compute node group", &created.compute_node_group_arn, false),
        ("queue", &created.queue_arn, false),
    ] {
        let got = listed_tags(&router, arn).await;
        assert_eq!(
            got.contains_key("only-on"),
            expected_present,
            "case {kind:?} isolation: expected the key present={expected_present}, got {got:?}"
        );
    }

    // The resource's own reader still answers, so tagging changed nothing else about it.
    let (status, body) = call(
        &router,
        "GetCluster",
        &json!({"clusterIdentifier": &created.cluster_id}),
    )
    .await;
    assert_eq!(status, 200, "case GetCluster after tagging: got {body}");
    assert_eq!(
        body["cluster"]["status"],
        json!("ACTIVE"),
        "case GetCluster after tagging: expected the cluster still ACTIVE, got {body}"
    );
}

#[tokio::test]
async fn a_tag_write_past_the_resource_tag_quota_is_refused_and_persists_nothing() {
    let router = emulator();
    let created = tagged_resources(&router, &json!({"created": "by-the-create-call"})).await;
    let arn = created.cluster_arn.clone();

    // The resource starts with the one tag its create carried, so filling it to the
    // ceiling takes one fewer write.
    let fill = bulk_tags("fill", MAX_TAGS_PER_RESOURCE - 1);
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": &arn, "tags": fill}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case up to the ceiling exactly: expected HTTP 200 for {} tags, got {status} with \
         {body:?}",
        MAX_TAGS_PER_RESOURCE
    );
    let at_ceiling = listed_tags(&router, &arn).await;
    assert_eq!(
        at_ceiling.len(),
        MAX_TAGS_PER_RESOURCE,
        "case up to the ceiling exactly: expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        at_ceiling.len()
    );

    // Editing a key the resource already carries does not grow the set, so it is accepted
    // at the ceiling; adding any new key is not.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": &arn, "tags": {"fill-000": "edited"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an edit at the ceiling: expected HTTP 200, got {status} with {body:?}"
    );
    let edited = listed_tags(&router, &arn).await;
    assert_eq!(
        edited.get("fill-000").map(String::as_str),
        Some("edited"),
        "case an edit at the ceiling: expected the new value, got {edited:?}"
    );
    assert_eq!(
        edited.len(),
        MAX_TAGS_PER_RESOURCE,
        "case an edit at the ceiling: expected the set unchanged in size, got {}",
        edited.len()
    );

    // Rows: how many NEW keys a write at the ceiling asks for, against what the service
    // answers. `RequestTagMap` is modelled @length(min: 1, max: 200), so a request
    // carrying more than 200 entries is refused at the boundary (400) before the quota is
    // ever consulted, and one carrying none is refused the same way.
    let past: Vec<(&str, usize, u16)> = vec![
        ("one new key past the ceiling", 1, 402),
        ("a handful past the ceiling", 7, 402),
        ("a request at the modelled maximum", 200, 402),
        ("a request one past the modelled maximum", 201, 400),
    ];
    for (case, new_keys, expected_status) in past {
        let request = bulk_tags(case, new_keys);
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": &arn, "tags": request}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {new_keys} new keys, got \
             {status} with {body:?}"
        );
        if expected_status == 402 {
            assert_eq!(
                body["__type"],
                json!("ServiceQuotaExceededException"),
                "case {case:?}: expected the modelled quota error, got {body:?}"
            );
            assert_eq!(
                body["message"],
                json!(format!(
                    "The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} \
                     user tags."
                )),
                "case {case:?}: got {body:?}"
            );
        }
        // Refused whole: not one of the request's keys was written.
        let after = listed_tags(&router, &arn).await;
        assert_eq!(
            after, edited,
            "case {case:?}: expected the stored tags unchanged, got {after:?}"
        );
    }

    // A request with no tags at all is refused by the modelled minimum, not the quota.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": &arn, "tags": {}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an empty tag map: expected HTTP 400 from the modelled minimum, got {status} \
         with {body:?}"
    );

    // Freeing a key makes room for exactly one more, so the ceiling is a bound on the
    // stored set and not a one-shot gate.
    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({"resourceArn": &arn, "tagKeys": ["fill-000"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case freeing one key: expected HTTP 200, got {status} with {body:?}"
    );
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": &arn, "tags": {"reused-room": "yes"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case reusing the freed room: expected HTTP 200, got {status} with {body:?}"
    );
    let reused = listed_tags(&router, &arn).await;
    assert_eq!(
        reused.len(),
        MAX_TAGS_PER_RESOURCE,
        "case reusing the freed room: expected {MAX_TAGS_PER_RESOURCE} tags, got {reused:?}"
    );

    // The ceiling holds for every taggable kind, on a resource the rows above never
    // touched: a single request past it is refused and stores nothing.
    for (kind, arn) in [
        ("compute node group", &created.compute_node_group_arn),
        ("queue", &created.queue_arn),
    ] {
        let too_many = bulk_tags("bulk", MAX_TAGS_PER_RESOURCE + 1);
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "tags": too_many}),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {kind:?} one request past the ceiling: expected HTTP 402, got {status} with \
             {body:?}"
        );
        let after = listed_tags(&router, arn).await;
        assert_eq!(
            after,
            [("created".to_string(), "by-the-create-call".to_string())]
                .into_iter()
                .collect::<BTreeMap<String, String>>(),
            "case {kind:?} one request past the ceiling: expected only the create's tag, got \
             {after:?}"
        );
    }

    // The ceiling is a bound on the RESOURCE, so the create path that writes `tags`
    // answers it the same way. Rows: the tag count a create carries, against the status
    // the service answers with — at the ceiling it is created and carries all of them,
    // past it the create is refused and no resource of that name exists.
    for (case, tag_count, expected_status) in [
        ("a create at the ceiling", MAX_TAGS_PER_RESOURCE, 200),
        (
            "a create one past the ceiling",
            MAX_TAGS_PER_RESOURCE + 1,
            402,
        ),
    ] {
        let name = format!("tags-on-create-{tag_count}");
        let (status, body) = call(
            &router,
            "CreateCluster",
            &json!({
                "clusterName": &name,
                "scheduler": {"type": "SLURM", "version": "24.11"},
                "size": "SMALL",
                "networking": {"subnetIds": ["subnet-0123456789abcdef0"]},
                "tags": bulk_tags("created", tag_count),
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {tag_count} tags on the \
             create, got {status} with {body:?}"
        );
        if expected_status == 200 {
            let created_arn = string_at(&body, &["cluster", "arn"]);
            let stored = listed_tags(&router, &created_arn).await;
            assert_eq!(
                stored.len(),
                tag_count,
                "case {case:?}: expected {tag_count} tags stored, got {stored:?}"
            );
        } else {
            assert_eq!(
                body["__type"],
                json!("ServiceQuotaExceededException"),
                "case {case:?}: expected the modelled quota error, got {body:?}"
            );
            let (status, refused) = call(
                &router,
                "GetCluster",
                &json!({"clusterIdentifier": &name}),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected no cluster to have been created, got {status} with \
                 {refused:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_tagging_operations_refuse_an_arn_no_resource_carries() {
    let router = emulator();
    let created = tagged_resources(&router, &json!({"created": "by-the-create-call"})).await;
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Rows: an ARN naming each resource kind that does not exist — one inside a cluster
    // that DOES exist, so the refusal is about the child and not its parent — plus an ARN
    // in another region, which names no resource this account's store holds either.
    let absent: Vec<(&str, String, &str)> = vec![
        (
            "cluster",
            format!("arn:aws:pcs:{REGION}:{account}:cluster/pcs_0000000000000000"),
            "AWS PCS couldn't find the cluster you specified.",
        ),
        (
            "queue",
            format!(
                "arn:aws:pcs:{REGION}:{account}:cluster/{}/queue/pcs_0000000000000000",
                created.cluster_id
            ),
            "AWS PCS couldn't find the queue you specified.",
        ),
        (
            "computenodegroup",
            format!(
                "arn:aws:pcs:{REGION}:{account}:cluster/{}/computenodegroup/pcs_0000000000000000",
                created.cluster_id
            ),
            "AWS PCS couldn't find the compute node group you specified.",
        ),
        (
            "cluster",
            created
                .cluster_arn
                .replace(REGION, "eu-west-1"),
            "AWS PCS couldn't find the cluster you specified.",
        ),
        // A child id that EXISTS, under a cluster id that does not: a resource is reached
        // by exactly the ARN the service minted for it, never by one that merely ends in
        // the same id.
        (
            "queue",
            created.queue_arn.replace(
                &created.cluster_id,
                "pcs_0000000000000000",
            ),
            "AWS PCS couldn't find the queue you specified.",
        ),
        (
            "computenodegroup",
            created.compute_node_group_arn.replace(
                &created.cluster_id,
                "pcs_0000000000000000",
            ),
            "AWS PCS couldn't find the compute node group you specified.",
        ),
    ];

    for (resource_type, arn, message) in &absent {
        for (operation, request) in [
            (
                "TagResource",
                json!({"resourceArn": arn, "tags": {"env": "prod"}}),
            ),
            ("ListTagsForResource", json!({"resourceArn": arn})),
            (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": ["env"]}),
            ),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {operation} on an absent {resource_type}: expected HTTP 404, got {status} \
                 with {body:?}"
            );
            assert_eq!(
                body["__type"],
                json!("ResourceNotFoundException"),
                "case {operation} on an absent {resource_type}: got {body:?}"
            );
            assert_eq!(
                body["message"],
                json!(message),
                "case {operation} on an absent {resource_type}: expected {message:?}, got {body:?}"
            );
            assert_eq!(
                body["resourceType"],
                json!(resource_type),
                "case {operation} on an absent {resource_type}: got {body:?}"
            );
        }
    }

    // A resource that existed and is gone is in the same class: a cluster of its own,
    // with no children to delete first, carries tags while it lives and none once the
    // clock has removed it.
    let (status, body) = call(
        &router,
        "CreateCluster",
        &json!({
            "clusterName": "gone-cluster",
            "scheduler": {"type": "SLURM", "version": "24.11"},
            "size": "SMALL",
            "networking": {"subnetIds": ["subnet-0123456789abcdef0"]},
            "tags": {"created": "by-the-create-call"},
        }),
    )
    .await;
    assert_eq!(status, 200, "case CreateCluster gone-cluster: got {body}");
    let gone_arn = string_at(&body, &["cluster", "arn"]);
    wait_past_the_owed_transition().await;
    let while_alive = listed_tags(&router, &gone_arn).await;
    assert_eq!(
        while_alive,
        [("created".to_string(), "by-the-create-call".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case a live cluster's tags: got {while_alive:?}"
    );

    let (status, body) = call(
        &router,
        "DeleteCluster",
        &json!({"clusterIdentifier": "gone-cluster"}),
    )
    .await;
    assert_eq!(status, 200, "case DeleteCluster: got {body}");
    wait_past_the_owed_transition().await;

    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": &gone_arn}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case ListTagsForResource on a deleted cluster: expected HTTP 404, got {status} with \
         {body:?}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "case ListTagsForResource on a deleted cluster: got {body:?}"
    );
}
