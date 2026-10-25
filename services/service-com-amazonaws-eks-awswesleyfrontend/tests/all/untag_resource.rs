// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UntagResource`: the delete half of the EKS tag slice,
//! driven over the wire through the same `MultiServiceRouter` the production
//! binary mounts.
//!
//! RULE: `UntagResource` removes from the resource's STORED tag map exactly the
//! keys the request names and nothing else — a key the request does not name
//! survives, and a key the resource does not carry is removed vacuously — so
//! `ListTagsForResource` and `DescribeCluster` then report the remaining map and a
//! repeated call answers the same 200.
//!
//! Every suite here asserts that rule over a TABLE of inputs rather than one
//! recorded request: the rows differ along what the rule generalises over (how
//! many of the stored keys are named, whether a named key is present at all, the
//! characters a key carries, and the modelled 1..50 size of `tagKeys`), and each
//! assertion names its case and prints the actual value.

use std::collections::BTreeMap;

use service_com_amazonaws_eks_awswesleyfrontend::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, delete, describe, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

type Tags = BTreeMap<String, String>;

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `DELETE /tags/{resourceArn}?tagKeys=…`, one `tagKeys` parameter per key, which
/// is how an SDK sends a list-typed `@httpQuery` member
/// (`smithy_query query="operation" operation="UntagResource"`).
///
/// A key travels as a query VALUE, so it is percent-encoded with the harness's
/// [`encode_label`]: that escapes every byte outside `A-Za-z0-9`, a superset of
/// what a query value must escape, and the server percent-decodes it either way.
async fn untag(router: &MultiServiceRouter, arn: &str, tag_keys: &[&str]) -> Answer {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("/tags/{}?{query}", encode_label(arn));
    call(router, REGION, "DELETE", &uri, "").await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        "",
    )
    .await
}

async fn tag(router: &MultiServiceRouter, arn: &str, map: &Tags) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        &serde_json::json!({ "tags": map }).to_string(),
    )
    .await
}

/// Create a cluster carrying `create_tags` and hand back its ARN.
async fn cluster_with_tags(router: &MultiServiceRouter, name: &str, create_tags: &Tags) -> String {
    let mut request = minimal_request(name);
    if !create_tags.is_empty() {
        request["tags"] = serde_json::to_value(create_tags).expect("tags serialize");
    }
    let created = create(router, &request).await;
    assert_eq!(
        created.status, 200,
        "setup create {name:?}: expected 200, got {created:?}"
    );
    created.cluster()["arn"]
        .as_str()
        .expect("a created cluster carries an arn")
        .to_owned()
}

// ---------------------------------------------------------------------------
// RULE: the named keys leave the stored map and every other key stays.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untagging_removes_only_the_named_keys() {
    // A map at the modelled 1..50 ceiling, so one request may name every key it
    // holds (`tagKeys` is itself bounded 1..50).
    let at_ceiling: Tags = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let every_ceiling_key: Vec<&str> = at_ceiling.keys().map(String::as_str).collect();

    // (case, the tags the cluster is created with, the keys to delete, what is left)
    let cases: Vec<(&str, Tags, Vec<&str>, Tags)> = vec![
        (
            "one key of several",
            tags(&[("env", "dev"), ("team", "platform")]),
            vec!["env"],
            tags(&[("team", "platform")]),
        ),
        (
            "the only key, leaving an empty map",
            tags(&[("env", "dev")]),
            vec!["env"],
            Tags::new(),
        ),
        (
            "every key of several",
            tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            vec!["a", "b", "c"],
            Tags::new(),
        ),
        (
            "a key the resource does not carry leaves the map alone",
            tags(&[("env", "dev")]),
            vec!["ghost"],
            tags(&[("env", "dev")]),
        ),
        (
            "present and absent keys together",
            tags(&[("env", "dev"), ("keep", "me")]),
            vec!["env", "ghost"],
            tags(&[("keep", "me")]),
        ),
        (
            "a key whose characters must be percent-encoded in the query string",
            tags(&[("cost:center", "42"), ("path/like", "yes"), ("keep", "me")]),
            vec!["cost:center", "path/like"],
            tags(&[("keep", "me")]),
        ),
        (
            "a resource with no tags at all",
            Tags::new(),
            vec!["ghost"],
            Tags::new(),
        ),
        (
            "the modelled 1..50 high bound: every key of a map at the ceiling",
            at_ceiling.clone(),
            every_ceiling_key,
            Tags::new(),
        ),
    ];

    for (index, (case, create_tags, tag_keys, expected)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("untagged-{index}");
        let arn = cluster_with_tags(&router, &name, create_tags).await;

        // A second cluster carrying the SAME keys: deleting tags from one resource
        // must not touch another's map.
        let bystander = format!("bystander-{index}");
        let bystander_arn = cluster_with_tags(&router, &bystander, create_tags).await;

        let answer = untag(&router, &arn, tag_keys).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from UntagResource for {tag_keys:?}, got {answer:?}"
        );

        let expected_json = serde_json::to_value(expected).expect("tags serialize");
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {listed:?}"
        );
        assert_eq!(
            listed.body["tags"], expected_json,
            "case {case:?}: expected the remaining map {expected_json:?}, got {:?}",
            listed.body["tags"]
        );

        // The same remaining map is what the cluster itself reports.
        let described = describe(&router, &name).await;
        assert_eq!(
            described.cluster()["tags"], expected_json,
            "case {case:?}: DescribeCluster must report the remaining map, got {:?}",
            described.cluster()["tags"]
        );

        let untouched = serde_json::to_value(create_tags).expect("tags serialize");
        let bystander_tags = list_tags(&router, &bystander_arn).await;
        assert_eq!(
            bystander_tags.body["tags"], untouched,
            "case {case:?}: another resource's map must be untouched, expected \
             {untouched:?}, got {:?}",
            bystander_tags.body["tags"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (the same one, as an invariant): the request asks for an ABSENCE, so
// repeating it changes nothing, and `TagResource` puts a deleted key back.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untagging_is_idempotent_and_a_deleted_key_can_be_tagged_again() {
    // Rows differ along how many keys the repeated delete names and how many of
    // them the resource still carries after the first call.
    let cases: Vec<(&str, Tags, Vec<&str>, Tags)> = vec![
        (
            "one key, deleted twice",
            tags(&[("env", "dev"), ("keep", "me")]),
            vec!["env"],
            tags(&[("keep", "me")]),
        ),
        (
            "several keys, deleted twice",
            tags(&[("a", "1"), ("b", "2"), ("keep", "me")]),
            vec!["a", "b"],
            tags(&[("keep", "me")]),
        ),
        (
            "the whole map, deleted twice",
            tags(&[("a", "1"), ("b", "2")]),
            vec!["a", "b"],
            Tags::new(),
        ),
    ];

    for (index, (case, create_tags, tag_keys, expected)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("idempotent-{index}");
        let arn = cluster_with_tags(&router, &name, create_tags).await;
        let expected_json = serde_json::to_value(expected).expect("tags serialize");

        for round in 1..=2 {
            let answer = untag(&router, &arn, tag_keys).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: round {round} of UntagResource must answer 200, got {answer:?}"
            );
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.body["tags"], expected_json,
                "case {case:?}: round {round} must leave {expected_json:?}, got {:?}",
                listed.body["tags"]
            );
        }

        // ROUND TRIP: a key that was deleted is a key `TagResource` can add back,
        // with a fresh value, and nothing else moves.
        let re_added: Tags = tag_keys
            .iter()
            .map(|key| ((*key).to_string(), "again".to_string()))
            .collect();
        let tagged = tag(&router, &arn, &re_added).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected 200 re-tagging {re_added:?}, got {tagged:?}"
        );
        let mut after: Tags = expected.clone();
        after.extend(re_added.clone());
        let after_json = serde_json::to_value(&after).expect("tags serialize");
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.body["tags"], after_json,
            "case {case:?}: expected {after_json:?} after re-tagging, got {:?}",
            listed.body["tags"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `UntagResource` classifies its `resourceArn` the way the other two tag
// operations do — a value that is not a resource ARN of this service is a 400
// `BadRequestException` refused before any lookup, and a well-formed ARN no
// stored resource owns is a 404 `NotFoundException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untagging_classifies_a_resource_arn_as_the_other_tag_operations_do() {
    let router = emulator();

    // A cluster created and then deleted: its ARN is well-formed and owned by
    // nothing.
    let deleted_arn = cluster_with_tags(&router, "untag-gone-soon", &tags(&[("env", "dev")])).await;
    let removed = delete(&router, "untag-gone-soon").await;
    assert_eq!(removed.status, 200, "setup delete: got {removed:?}");

    // (case, the value sent as `resourceArn`, the status and error it earns)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a bare cluster name, which is not an ARN at all",
            "untag-gone-soon".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "an ARN of another vendor",
            format!("arn:aws:s3:{REGION}:000000000000:bucket/tags"),
            400,
            "BadRequestException",
        ),
        (
            "an ARN missing its resource field",
            format!("arn:aws:eks:{REGION}:000000000000"),
            400,
            "BadRequestException",
        ),
        (
            "a cluster ARN for a name never created",
            format!("arn:aws:eks:{REGION}:000000000000:cluster/never"),
            404,
            "NotFoundException",
        ),
        (
            "the ARN of a deleted cluster",
            deleted_arn,
            404,
            "NotFoundException",
        ),
        (
            "a nodegroup ARN, a resource kind this slice does not hold",
            format!("arn:aws:eks:{REGION}:000000000000:nodegroup/c/n/uuid"),
            404,
            "NotFoundException",
        ),
    ];

    for (case, arn, status, error) in &cases {
        let answer = untag(&router, arn, &["env"]).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status} from UntagResource, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            *error,
            "case {case:?}: expected {error}, got {answer:?}"
        );

        // The three tag operations agree on the class of the same value.
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.status, *status,
            "case {case:?}: ListTagsForResource must answer the same HTTP {status}, \
             got {listed:?}"
        );
    }
}
