// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over the wire.
//!
//! RULE under test: a tag key in the namespace AWS reserves for its own tags — one
//! beginning `aws:` — is not the caller's to write or to remove, so EVERY operation
//! of this service that accepts caller-supplied tag keys (`TagResource`,
//! `UntagResource`, and the `tags` member of `CreateCluster` and `CreateStream`)
//! refuses the WHOLE request with the modelled `ValidationException` (HTTP 400,
//! `reason` `fieldValidationFailed`, `fieldList` naming the member) and has no
//! effect: no tag written, no tag removed, no resource created. A key that merely
//! resembles the prefix is an ordinary customer key and is served.
//!
//! `TagKey`'s modelled pattern `^[a-zA-Z0-9_.:/=+\-@ ]*$` (`smithy_query(query="shape",
//! shape="TagKey")`) admits the `:` such a key needs, so the boundary validator passes
//! these keys through and the rule is the service's own to enforce.
//!
//! The tables span the class the rule generalises over: the OPERATION that carries the
//! keys, the KIND of resource addressed (a cluster and a stream of a cluster, the two
//! tagged resources of this service), the SPELLING of the reserved key (the prefix
//! alone, a short one, a deeply qualified AWS-written key, case variants of the prefix,
//! a key at `TagKey`'s published maximum), the PLACEMENT of the reserved key in the
//! request (alone, beside ordinary keys, beside a key the resource already holds) and
//! the near misses on the other side of the boundary (`aws` with no colon, a colon
//! elsewhere in the key, the prefix not at the start). The reported case — a cluster
//! tagged `aws:cloudformation:stack-name` — is one row.
//!
//! The client is the shared `Dsql` harness in `harness.rs`; `UntagResource`'s query
//! encoding is the one the `resource_tags` suite already spells.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{wait_for_the_clock, Answer, Dsql, TAG_KEY_MAX};
use crate::resource_tags::untag;

/// The prefix AWS reserves on tag keys: tags under it are created and maintained by
/// AWS itself (the stack a resource was provisioned by, for instance), so a caller
/// can neither write nor remove one. Well-established AWS tagging semantics, which
/// the model cannot express — hence this suite.
const RESERVED_PREFIX: &str = "aws:";

/// The `ValidationExceptionReason` a member-value refusal carries
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// Keys INSIDE the reserved namespace, spanning the spellings one takes: the prefix
/// alone, the shortest key under it, the deeply qualified key AWS itself writes, the
/// prefix in other cases (it names a namespace, not one spelling), and a reserved key
/// at `TagKey`'s published 128-character maximum.
fn reserved_keys() -> Vec<String> {
    vec![
        "aws:cloudformation:stack-name".to_string(),
        RESERVED_PREFIX.to_string(),
        format!("{RESERVED_PREFIX}x"),
        "AWS:CLOUDFORMATION:STACK-ID".to_string(),
        "Aws:owner".to_string(),
        format!(
            "{RESERVED_PREFIX}{}",
            "k".repeat(TAG_KEY_MAX - RESERVED_PREFIX.len())
        ),
    ]
}

/// Keys OUTSIDE the namespace that come close to it — the prefix without its colon,
/// a colon somewhere other than after `aws`, the prefix not at the start, and a
/// single-character key at `TagKey`'s published minimum. Every one of these is a
/// character sequence `TagKey`'s pattern allows, and an ordinary customer key.
fn accepted_keys() -> Vec<String> {
    [
        "aws",
        "awsome",
        "aws-cloudformation",
        "aws.cloudformation",
        "aw:s",
        "my-aws:key",
        " aws:x",
        "a",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// A tag map as the restJson1 body carries it.
fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// A tag map from `(key, value)` pairs.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tag map an answer reports: an absent member and an empty map are the same tag
/// set (`GetCluster` omits the member for an untagged resource).
fn reported_tags(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .get("tags")
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A `targetDefinition` naming a Kinesis destination, as `CreateStream` requires
/// (`KinesisTargetDefinition`: a `streamArn` and a `roleArn`).
fn kinesis_target(region: &str) -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:{region}:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// Assert one refusal: the modelled `ValidationException`, naming the input member
/// the caller has to fix and the key that failed.
fn assert_reserved_key_refusal(case: &str, answer: &Answer, member: &str, key: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a reserved tag key is refused with HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .error
            .as_deref()
            .is_some_and(|code| code.contains("ValidationException")),
        "case {case:?}: the wire code must be ValidationException, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.str("reason"),
        REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: a member-value refusal reports {REASON_FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    assert!(
        answer.message().contains(key),
        "case {case:?}: the message must name the refused key {key:?}, got {:?}",
        answer.message()
    );
    let field_list = answer.get("fieldList");
    let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        fields.iter().any(|field| {
            field.get("name").and_then(Value::as_str) == Some(member)
                && field
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
        }),
        "case {case:?}: fieldList must name {member} with a message saying why, got {field_list:?}"
    );
}

/// One target of a tag write: the ARN addressed, how the case names it, and the tags
/// it holds before the refused call.
struct Target {
    kind: &'static str,
    arn: String,
    held: BTreeMap<String, String>,
}

/// A cluster and a stream of it, each carrying tags — the two tagged resources of
/// this service, with the clock's transitions already settled.
async fn tagged_targets(dsql: &Dsql, region: &str) -> Vec<Target> {
    let cluster_tags = tags(&[("env", "prod"), ("owner", "team-a")]);
    let created = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "tags": tag_object(&cluster_tags),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must answer 200, got {} {:?}",
        created.status, created.body
    );
    let cluster_identifier = created.str("identifier");
    // The cluster has to be past the clock's `CREATING -> ACTIVE` transition before
    // it can take a stream.
    wait_for_the_clock().await;

    let stream_tags = tags(&[("scope", "cdc")]);
    let stream = dsql
        .create_stream(
            &cluster_identifier,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": kinesis_target(region),
                "tags": tag_object(&stream_tags),
            }),
        )
        .await;
    assert_eq!(
        stream.status, 200,
        "CreateStream must answer 200, got {} {:?}",
        stream.status, stream.body
    );
    wait_for_the_clock().await;

    vec![
        Target {
            kind: "a cluster",
            arn: created.str("arn"),
            held: cluster_tags,
        },
        Target {
            kind: "a stream of a cluster",
            arn: stream.str("arn"),
            held: stream_tags,
        },
    ]
}

/// RULE: `TagResource` refuses the whole request when ANY key it carries is in the
/// reserved namespace, whichever resource the ARN names and whatever else the map
/// carries, and the resource's tags are left exactly as they were.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_writes_nothing() {
    let region = "us-east-1";
    let dsql = Dsql::in_region(region);
    let targets = tagged_targets(&dsql, region).await;

    for target in &targets {
        for key in reserved_keys() {
            // The reserved key alone; beside an ordinary NEW key; and beside a key
            // the resource already holds with a different value — so a refusal that
            // leaked would show up as either an addition or an overwrite.
            let placements: Vec<(&str, BTreeMap<String, String>)> = vec![
                ("alone", tags(&[(key.as_str(), "mine")])),
                (
                    "beside a new key",
                    tags(&[(key.as_str(), "mine"), ("added", "later")]),
                ),
                (
                    "beside a key the resource holds",
                    tags(&[(key.as_str(), "mine"), ("env", "overwritten")]),
                ),
            ];
            for (placement, request) in placements {
                let case = format!(
                    "TagResource on {} with {key:?} {placement}",
                    target.kind
                );
                let answer = dsql
                    .tag_resource(&target.arn, json!({"tags": tag_object(&request)}))
                    .await;
                assert_reserved_key_refusal(&case, &answer, "tags", &key);

                let listed = dsql.list_tags_for_resource(&target.arn).await;
                assert_eq!(
                    reported_tags(&listed),
                    target.held,
                    "case {case:?}: a refused write must leave the tags unchanged, got {:?}",
                    listed.get("tags")
                );
            }
        }
    }
}

/// RULE: `UntagResource` refuses the whole request when any key it names is in the
/// reserved namespace — a reserved tag is no more the caller's to delete than to
/// write — and removes nothing.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    let region = "us-east-1";
    let dsql = Dsql::in_region(region);
    let targets = tagged_targets(&dsql, region).await;

    for target in &targets {
        for key in reserved_keys() {
            let held_key = target
                .held
                .keys()
                .next()
                .expect("each target carries at least one tag")
                .clone();
            let requests: Vec<(&str, Vec<String>)> = vec![
                ("alone", vec![key.clone()]),
                (
                    "beside a key the resource holds",
                    vec![key.clone(), held_key],
                ),
            ];
            for (placement, named) in requests {
                let case = format!(
                    "UntagResource on {} with {key:?} {placement}",
                    target.kind
                );
                let answer = untag(&dsql, &target.arn, &named).await;
                assert_reserved_key_refusal(&case, &answer, "tagKeys", &key);

                let listed = dsql.list_tags_for_resource(&target.arn).await;
                assert_eq!(
                    reported_tags(&listed),
                    target.held,
                    "case {case:?}: a refused removal must leave the tags unchanged, got {:?}",
                    listed.get("tags")
                );
            }
        }
    }
}

/// RULE: the `tags` a create carries go through the same rule, so `CreateCluster`
/// and `CreateStream` refuse a reserved key and mint nothing at all.
#[tokio::test]
async fn a_create_carrying_a_reserved_key_refuses_and_mints_nothing() {
    let region = "eu-west-1";
    let dsql = Dsql::in_region(region);
    let host_cluster = dsql.create_settled().await;

    let before = dsql.list("?max-results=200").await;
    let clusters_before = before
        .get("clusters")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();

    for key in reserved_keys() {
        for (placement, request) in [
            ("alone", tags(&[(key.as_str(), "mine")])),
            (
                "beside an ordinary key",
                tags(&[(key.as_str(), "mine"), ("env", "prod")]),
            ),
        ] {
            let case = format!("CreateCluster with {key:?} {placement}");
            let answer = dsql
                .create(json!({
                    "deletionProtectionEnabled": false,
                    "tags": tag_object(&request),
                }))
                .await;
            assert_reserved_key_refusal(&case, &answer, "tags", &key);
            assert!(
                answer.str("identifier").is_empty(),
                "case {case:?}: a refused create must hand back no cluster, got {:?}",
                answer.body
            );

            let case = format!("CreateStream with {key:?} {placement}");
            let answer = dsql
                .create_stream(
                    &host_cluster,
                    json!({
                        "format": "JSON",
                        "ordering": "UNORDERED",
                        "targetDefinition": kinesis_target(region),
                        "tags": tag_object(&request),
                    }),
                )
                .await;
            assert_reserved_key_refusal(&case, &answer, "tags", &key);
            assert!(
                answer.str("arn").is_empty(),
                "case {case:?}: a refused create must hand back no stream, got {:?}",
                answer.body
            );
        }
    }

    // Nothing was minted: the store holds the clusters it held, and the host cluster
    // holds no stream.
    let after = dsql.list("?max-results=200").await;
    let clusters_after = after
        .get("clusters")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        clusters_after, clusters_before,
        "every refused CreateCluster must leave the store at {clusters_before} clusters, got {:?}",
        after.get("clusters")
    );
    let streams = dsql.list_streams(&host_cluster, "").await;
    assert_eq!(
        streams
            .get("streams")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "every refused CreateStream must leave the cluster with no stream, got {:?}",
        streams.get("streams")
    );
}

/// RULE: only the reserved namespace is refused — a key that merely resembles the
/// prefix is an ordinary customer key, so it is created, reported, merged and
/// removed exactly as any other key is.
#[tokio::test]
async fn a_key_that_only_resembles_the_reserved_prefix_is_served() {
    let region = "us-east-2";
    let dsql = Dsql::in_region(region);

    for key in accepted_keys() {
        let case = format!("the key {key:?}");
        let created = dsql
            .create(json!({
                "deletionProtectionEnabled": false,
                "tags": tag_object(&tags(&[(key.as_str(), "at-create")])),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateCluster must accept it, got {} {:?}",
            created.status, created.body
        );
        let identifier = created.str("identifier");
        let arn = created.str("arn");
        wait_for_the_clock().await;

        let fetched = dsql.get(&identifier).await;
        assert_eq!(
            reported_tags(&fetched),
            tags(&[(key.as_str(), "at-create")]),
            "case {case:?}: GetCluster must report the tag it was created with, got {:?}",
            fetched.get("tags")
        );

        let tagged = dsql
            .tag_resource(&arn, json!({"tags": tag_object(&tags(&[(key.as_str(), "merged")]))}))
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must accept it, got {} {:?}",
            tagged.status, tagged.body
        );
        let listed = dsql.list_tags_for_resource(&arn).await;
        assert_eq!(
            reported_tags(&listed),
            tags(&[(key.as_str(), "merged")]),
            "case {case:?}: the merged value must be the one reported, got {:?}",
            listed.get("tags")
        );

        let removed = untag(&dsql, &arn, std::slice::from_ref(&key)).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: UntagResource must accept it, got {} {:?}",
            removed.status, removed.body
        );
        let after = dsql.list_tags_for_resource(&arn).await;
        assert_eq!(
            reported_tags(&after),
            BTreeMap::new(),
            "case {case:?}: the removed key must be gone, got {:?}",
            after.get("tags")
        );
    }
}
