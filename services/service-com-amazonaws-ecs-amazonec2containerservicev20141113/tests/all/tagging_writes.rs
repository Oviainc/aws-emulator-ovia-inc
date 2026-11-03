// SPDX-License-Identifier: CC0-1.0

//! WRITING an ECS resource's tags through `TagResource` and `UntagResource`, driven end to
//! end over HTTP.
//!
//! The rule under test: a tagging write acts on the ONE tag set the named resource holds.
//! `TagResource` leaves that set carrying the value the request gives each key it names,
//! keeps every key the request does not name, and refuses a write that would leave the
//! resource over the documented maximum of tags without writing anything; `UntagResource`
//! deletes exactly the keys it names and nothing else. Whichever kind of ECS resource the
//! ARN names, the result is what `ListTagsForResource` and the resource's own describe
//! report afterwards. An ARN that names no held resource is refused with the modelled
//! error its KIND already earns elsewhere in this service, and an ARN that is not a
//! taggable ECS resource ARN at all is refused as the bad parameter it is.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, string, ACCOUNT, SIGNED_REGION};
use crate::resource_tags::{create, listed, pairs, setup, Kind, CLUSTER, RUNNABLE_FAMILY};

/// The documented per-resource tag maximum, which the limit rows below sit on either side
/// of: "Maximum number of tags per resource - 50" (the `Tag` doc, and the `Tags` list's
/// own length constraint — `smithy_query(query="shape", shape="Tag,Tags")`).
const MAX_TAGS: usize = 50;

/// `count` distinct tags, `key0..` / `value0..`, for a row that needs a tag set of a
/// particular SIZE rather than of particular values.
fn many(count: usize) -> Value {
    (0..count)
        .map(|index| json!({ "key": format!("key{index}"), "value": format!("value{index}") }))
        .collect()
}

/// The `(key, value)` pairs of a `tags` list literal, sorted — what the rows below state
/// their expectation as.
fn expected(tags: &Value, case: &str) -> Vec<(String, String)> {
    pairs(
        tags.as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the row's tags are a list")),
        case,
    )
}

/// One `TagResource` call that must succeed.
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &Value, case: &str) {
    ok(
        router,
        "TagResource",
        json!({ "resourceArn": resource_arn, "tags": tags }),
        case,
    )
    .await;
}

/// One `UntagResource` call that must succeed.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, tag_keys: &Value, case: &str) {
    ok(
        router,
        "UntagResource",
        json!({ "resourceArn": resource_arn, "tagKeys": tag_keys }),
        case,
    )
    .await;
}

#[tokio::test]
async fn tagging_leaves_the_named_keys_set_and_every_other_key_untouched() {
    let router = service_with_setup().await;

    // The documented bounds a row sits on: `Tag$key` length 1..128, `Tag$value` 0..256
    // (`smithy_query(query="shape", shape="Tag")`).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    // The rows differ along the resource KIND, how many tags the resource already carries,
    // how many the request names (none, one, two, the documented maximum), whether each
    // named key is new or already stored, and the FORM of the key and value — the
    // punctuation the `Tag` pattern allows, an empty value, and each length bound.
    let rows: Vec<(&str, Kind, String, Value, Value, Value)> = vec![
        (
            "the first tag on an untagged cluster",
            Kind::Cluster,
            "first".to_string(),
            json!([]),
            json!([{ "key": "env", "value": "prod" }]),
            json!([{ "key": "env", "value": "prod" }]),
        ),
        (
            "a request naming no key at all changes nothing",
            Kind::Cluster,
            "nokeys".to_string(),
            json!([{ "key": "env", "value": "prod" }]),
            json!([]),
            json!([{ "key": "env", "value": "prod" }]),
        ),
        (
            "one key overwritten, the other left alone",
            Kind::Cluster,
            "overwrite".to_string(),
            json!([{ "key": "env", "value": "prod" }, { "key": "owner", "value": "team-a" }]),
            json!([{ "key": "env", "value": "staging" }]),
            json!([{ "key": "env", "value": "staging" }, { "key": "owner", "value": "team-a" }]),
        ),
        (
            "a task definition taking an empty value over a stored one, plus a new key",
            Kind::TaskDefinition,
            "emptied".to_string(),
            json!([{ "key": "env", "value": "prod" }]),
            json!([{ "key": "env", "value": "" }, { "key": "cost:center", "value": "cc-42" }]),
            json!([{ "key": "env", "value": "" }, { "key": "cost:center", "value": "cc-42" }]),
        ),
        (
            "a capacity provider tagged on both length bounds and the allowed punctuation",
            Kind::CapacityProvider,
            "bounds".to_string(),
            json!([]),
            json!([
                { "key": longest_key, "value": longest_value },
                { "key": "a b+c_d@e", "value": "x/y=z.1:2-3" },
            ]),
            json!([
                { "key": longest_key, "value": longest_value },
                { "key": "a b+c_d@e", "value": "x/y=z.1:2-3" },
            ]),
        ),
        (
            "a service re-tagged with the value it already carries",
            Kind::Service,
            "same".to_string(),
            json!([{ "key": "env", "value": "test" }]),
            json!([{ "key": "env", "value": "test" }]),
            json!([{ "key": "env", "value": "test" }]),
        ),
        (
            "keys differing only in case are different keys",
            Kind::Service,
            "cased".to_string(),
            json!([{ "key": "env", "value": "prod" }]),
            json!([{ "key": "Env", "value": "prod" }]),
            json!([{ "key": "env", "value": "prod" }, { "key": "Env", "value": "prod" }]),
        ),
        (
            "a container instance tagged up to the documented maximum",
            Kind::ContainerInstance,
            "i-0123456789abcdef0".to_string(),
            json!([]),
            many(MAX_TAGS),
            many(MAX_TAGS),
        ),
        (
            "a task whose one tag is overwritten while another is added",
            Kind::Task,
            "unused".to_string(),
            json!([{ "key": "owner", "value": "suite" }]),
            json!([{ "key": "owner", "value": "other" }, { "key": "team", "value": "platform" }]),
            json!([{ "key": "owner", "value": "other" }, { "key": "team", "value": "platform" }]),
        ),
    ];

    for (case, kind, name, initial, request, result) in &rows {
        let arn = create(&router, *kind, name, initial, case).await;
        tag(&router, &arn, request, case).await;

        let want = expected(result, case);
        let actual = listed(&router, &arn, case).await;
        assert_eq!(
            actual, want,
            "case {case:?}: expected {arn} to carry {want:?} after TagResource({request}), \
             got {actual:?}"
        );

        // Applying the same write again is the same tag set: a key's value is what the
        // request says it is, not something that accumulates per call.
        tag(&router, &arn, request, case).await;
        let again = listed(&router, &arn, case).await;
        assert_eq!(
            again, want,
            "case {case:?}: a second identical TagResource on {arn} must leave {want:?}, \
             got {again:?}"
        );
    }
}

#[tokio::test]
async fn untagging_deletes_exactly_the_named_keys() {
    let router = service_with_setup().await;

    // The rows differ along the resource KIND, how many keys the request names, and
    // whether each named key is stored, absent, or named twice — plus the case where
    // every key is removed and the case where there is nothing to remove.
    let three =
        json!([{ "key": "env", "value": "prod" }, { "key": "owner", "value": "team-a" }, { "key": "tier", "value": "web" }]);
    let rows: Vec<(&str, Kind, String, Value, Value, Value)> = vec![
        (
            "one key of three",
            Kind::Cluster,
            "oneof3".to_string(),
            three.clone(),
            json!(["owner"]),
            json!([{ "key": "env", "value": "prod" }, { "key": "tier", "value": "web" }]),
        ),
        (
            "two keys of three",
            Kind::Cluster,
            "twoof3".to_string(),
            three.clone(),
            json!(["env", "tier"]),
            json!([{ "key": "owner", "value": "team-a" }]),
        ),
        (
            "every key",
            Kind::Cluster,
            "allof3".to_string(),
            three.clone(),
            json!(["env", "owner", "tier"]),
            json!([]),
        ),
        (
            "a key the resource does not carry",
            Kind::TaskDefinition,
            "absentkey".to_string(),
            json!([{ "key": "env", "value": "prod" }]),
            json!(["ghost"]),
            json!([{ "key": "env", "value": "prod" }]),
        ),
        (
            "a key named twice in one request",
            Kind::CapacityProvider,
            "twice".to_string(),
            json!([{ "key": "env", "value": "prod" }, { "key": "owner", "value": "team-a" }]),
            json!(["env", "env"]),
            json!([{ "key": "owner", "value": "team-a" }]),
        ),
        (
            "a key whose stored value is empty",
            Kind::Service,
            "emptyvalue".to_string(),
            json!([{ "key": "env", "value": "" }, { "key": "owner", "value": "team-a" }]),
            json!(["env"]),
            json!([{ "key": "owner", "value": "team-a" }]),
        ),
        (
            "a key differing only in case leaves the stored one",
            Kind::Service,
            "untagcased".to_string(),
            json!([{ "key": "env", "value": "prod" }]),
            json!(["ENV"]),
            json!([{ "key": "env", "value": "prod" }]),
        ),
        (
            "nothing to remove on an untagged container instance",
            Kind::ContainerInstance,
            "i-0fedcba9876543210".to_string(),
            json!([]),
            json!(["env"]),
            json!([]),
        ),
        (
            "the one tag of a task",
            Kind::Task,
            "unused".to_string(),
            json!([{ "key": "owner", "value": "suite" }]),
            json!(["owner"]),
            json!([]),
        ),
    ];

    for (case, kind, name, initial, keys, result) in &rows {
        let arn = create(&router, *kind, name, initial, case).await;
        untag(&router, &arn, keys, case).await;

        let want = expected(result, case);
        let actual = listed(&router, &arn, case).await;
        assert_eq!(
            actual, want,
            "case {case:?}: expected {arn} to carry {want:?} after UntagResource({keys}), \
             got {actual:?}"
        );

        // Removing a key that is already gone removes nothing, so the same request twice
        // leaves the same tag set.
        untag(&router, &arn, keys, case).await;
        let again = listed(&router, &arn, case).await;
        assert_eq!(
            again, want,
            "case {case:?}: a second identical UntagResource on {arn} must leave {want:?}, \
             got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_write_over_the_documented_maximum_is_refused_and_writes_nothing() {
    let router = service_with_setup().await;

    // Each row sits on or past the published bound of 50 tags per resource: how many tags
    // the cluster already carries, how many NEW keys the request adds, and whether the
    // resulting count is within the maximum.
    let rows: Vec<(&str, usize, usize, bool)> = vec![
        ("48 stored plus 2 new sits on the maximum", 48, 2, true),
        ("49 stored plus 1 new sits on the maximum", 49, 1, true),
        ("50 stored plus 1 new is one past it", MAX_TAGS, 1, false),
        ("49 stored plus 2 new is one past it", 49, 2, false),
        ("50 stored plus 10 new is well past it", MAX_TAGS, 10, false),
    ];

    for (case, stored, added, within) in &rows {
        let name = format!("limit{stored}x{added}");
        let initial = many(*stored);
        let arn = create(&router, Kind::Cluster, &name, &initial, case).await;

        // The added keys are NEW ones, numbered past the stored block, so the request's
        // own `tags` list stays within its modelled length while the merged set grows.
        let request: Value = (0..*added)
            .map(|index| {
                let key = format!("extra{index}");
                json!({ "key": key, "value": "added" })
            })
            .collect();

        if *within {
            tag(&router, &arn, &request, case).await;
            let actual = listed(&router, &arn, case).await;
            assert_eq!(
                actual.len(),
                stored + added,
                "case {case:?}: expected {arn} to carry {} tags, got {actual:?}",
                stored + added
            );
        } else {
            let (code, message) = refusal(
                &router,
                "TagResource",
                json!({ "resourceArn": arn, "tags": request }),
                case,
            )
            .await;
            assert_eq!(
                code, "LimitExceededException",
                "case {case:?}: expected the per-resource tag limit to be reported as \
                 LimitExceededException, got {code} ({message:?})"
            );
            assert!(
                message.contains(&MAX_TAGS.to_string()),
                "case {case:?}: the refusal must name the limit it reports, got {message:?}"
            );

            // A refused write leaves the stored tag set exactly as it was.
            let actual = listed(&router, &arn, case).await;
            let want = expected(&initial, case);
            assert_eq!(
                actual, want,
                "case {case:?}: a refused TagResource must write nothing, but {arn} now \
                 carries {actual:?} instead of {want:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_tagging_write_is_what_the_resources_own_describe_reports() {
    let router = service_with_setup().await;

    let cluster_arn = create(
        &router,
        Kind::Cluster,
        "described",
        &json!([{ "key": "env", "value": "prod" }]),
        "describe",
    )
    .await;

    tag(
        &router,
        &cluster_arn,
        &json!([{ "key": "owner", "value": "team-a" }, { "key": "env", "value": "staging" }]),
        "describe",
    )
    .await;

    // One tag set, two readers: the write is visible to the resource's own describe, not
    // just to the tag reader.
    let described = described_cluster_tags(&router, "described", "describe").await;
    let want = expected(
        &json!([{ "key": "env", "value": "staging" }, { "key": "owner", "value": "team-a" }]),
        "describe",
    );
    assert_eq!(
        described, want,
        "case \"describe\": expected DescribeClusters to report {want:?} after the write, \
         got {described:?}"
    );
    let listed_tags = listed(&router, &cluster_arn, "describe").await;
    assert_eq!(
        listed_tags, described,
        "case \"describe\": ListTagsForResource reported {listed_tags:?} but the cluster's \
         own describe reports {described:?}"
    );

    untag(&router, &cluster_arn, &json!(["env"]), "describe").await;
    let after = described_cluster_tags(&router, "described", "describe").await;
    let want = expected(&json!([{ "key": "owner", "value": "team-a" }]), "describe");
    assert_eq!(
        after, want,
        "case \"describe\": expected DescribeClusters to report {want:?} after the \
         removal, got {after:?}"
    );
}

#[tokio::test]
async fn tagging_writes_that_overlap_all_land_on_the_one_tag_set() {
    let router = service_with_setup().await;

    let arn = create(&router, Kind::Cluster, "overlap", &json!([]), "overlap").await;

    // Four writes of distinct keys, in flight together against the same resource: each one
    // merges into the single stored tag set, so none of them loses another's key.
    let case = "overlap";
    let keys = ["alpha", "beta", "gamma", "delta"];
    let writes = keys.map(|key| {
        let arn = arn.clone();
        let router = &router;
        async move {
            tag(
                router,
                &arn,
                &json!([{ "key": key, "value": "set" }]),
                case,
            )
            .await;
        }
    });
    let [first, second, third, fourth] = writes;
    tokio::join!(first, second, third, fourth);

    let actual = listed(&router, &arn, case).await;
    let want = expected(
        &keys
            .iter()
            .map(|key| json!({ "key": key, "value": "set" }))
            .collect::<Value>(),
        case,
    );
    assert_eq!(
        actual, want,
        "case {case:?}: every overlapping write must survive on {arn}: expected {want:?}, \
         got {actual:?}"
    );

    // And the reverse: four removals in flight together leave nothing behind.
    let removals = keys.map(|key| {
        let arn = arn.clone();
        let router = &router;
        async move {
            untag(router, &arn, &json!([key]), case).await;
        }
    });
    let [first, second, third, fourth] = removals;
    tokio::join!(first, second, third, fourth);

    let actual = listed(&router, &arn, case).await;
    assert!(
        actual.is_empty(),
        "case {case:?}: every overlapping removal must take effect on {arn}, got {actual:?}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_held_resource_is_refused_by_its_kind() {
    let router = service_with_setup().await;

    let ecs_arn = |resource: &str| format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:{resource}");

    // The same ARN classes the tag READER is refused on, now asked of both writes: a
    // well-formed ARN of each taggable kind naming nothing, then the values that are not a
    // taggable ECS resource ARN at all. The ERROR CODE is the requirement.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a cluster that was never created",
            ecs_arn("cluster/ghost"),
            "ClusterNotFoundException",
        ),
        (
            "a task-definition revision that was never registered",
            ecs_arn("task-definition/ghost:1"),
            "ClientException",
        ),
        (
            "a revision of a family that exists but was never registered",
            ecs_arn(&format!("task-definition/{RUNNABLE_FAMILY}:9")),
            "ClientException",
        ),
        (
            "a service that was never created",
            ecs_arn(&format!("service/{CLUSTER}/ghost")),
            "InvalidParameterException",
        ),
        (
            "a task that was never run",
            ecs_arn(&format!("task/{CLUSTER}/00000000000000000000000000000000")),
            "InvalidParameterException",
        ),
        (
            "a container instance that was never registered",
            ecs_arn(&format!(
                "container-instance/{CLUSTER}/00000000000000000000000000000000"
            )),
            "InvalidParameterException",
        ),
        (
            "a capacity provider that was never created",
            ecs_arn("capacity-provider/ghost"),
            "InvalidParameterException",
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{SIGNED_REGION}:{ACCOUNT}:some-queue"),
            "InvalidParameterException",
        ),
        (
            "an ECS resource kind that carries no tag set",
            ecs_arn(&format!(
                "container/{CLUSTER}/00000000000000000000000000000000/0123456789abcdef"
            )),
            "InvalidParameterException",
        ),
        (
            // Another partition's ARN carries another Region too, and this endpoint
            // resolves only its own Region's resources, so the ARN is what is refused.
            "an ECS ARN of another partition, for a cluster nothing created",
            format!("arn:aws-cn:ecs:cn-north-1:{ACCOUNT}:cluster/{CLUSTER}"),
            "InvalidParameterException",
        ),
        (
            "a value that is no ARN at all",
            CLUSTER.to_string(),
            "InvalidParameterException",
        ),
    ];

    for (case, resource_arn, want) in &rows {
        for (operation, request) in [
            (
                "TagResource",
                json!({ "resourceArn": resource_arn, "tags": [{ "key": "env", "value": "prod" }] }),
            ),
            (
                "UntagResource",
                json!({ "resourceArn": resource_arn, "tagKeys": ["env"] }),
            ),
        ] {
            let case = format!("{operation}: {case}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                &code, want,
                "case {case:?}: expected {want} for {resource_arn}, got {code} ({message:?})"
            );
            assert!(
                !message.is_empty(),
                "case {case:?}: every refusal carries a message, got {message:?}"
            );
        }
    }
}

/// The tags `DescribeClusters` reports for one cluster, as a sorted pair list. "If TAGS is
/// specified, the tags are included in the response."
async fn described_cluster_tags(
    router: &MultiServiceRouter,
    cluster: &str,
    case: &str,
) -> Vec<(String, String)> {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster], "include": ["TAGS"] }),
        case,
    )
    .await;
    let clusters = list(&body, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected exactly one described cluster, got {body}"
    );
    // The described record names the cluster the request asked for.
    let described = string(&clusters[0], "clusterName", case);
    assert_eq!(
        described, cluster,
        "case {case:?}: expected the described cluster to be {cluster}, got {described}"
    );
    pairs(&list(&clusters[0], "tags", case), case)
}

/// A router with this service mounted, the cluster the in-cluster resources live in, the
/// capacity a placed task needs, and one task-definition revision to run.
async fn service_with_setup() -> MultiServiceRouter {
    let router = crate::ecs_wire::service();
    setup(&router).await;
    router
}
