// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, end to end over the wire.
//!
//! RULE under test: `TagResource` MERGES the key/value pairs the request carries into
//! the tag set persisted on the resource its ARN names — a key the resource already
//! holds takes the request's value, a key the request does not name is left alone
//! (removal is `UntagResource`'s job) — the write is all-or-nothing and bounded by
//! `TagMap`'s published maximum, and an ARN that resolves to no stored resource is
//! refused (404) rather than tagged.
//!
//! The tables span the class the rule generalises over: the KIND of tagged resource the
//! ARN names (a cluster, and a stream of a cluster — the two resources of this service
//! that carry tags), the shape of the prior tag set (none, disjoint, colliding, both),
//! the size and characters of the keys and values (`TagKey` / `TagValue` at their
//! published bounds, every character class the pattern allows), the count (one, several,
//! forty, and the map at its ceiling) and the region. The recorded case — a cluster
//! created, then tagged — is one row.
//!
//! The client is the shared `Dsql` harness in `harness.rs`, which spells the
//! `/tags/{resourceArn}` routes, the `@httpLabel` encoding and the modelled tag bounds
//! once for every suite. The tags are read back through `GetCluster` and through
//! `ListTagsForResource`, the two implemented readers of the persisted map.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{
    absent_identifier, expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql,
    TAG_KEY_MAX, TAG_MAP_MAX, TAG_VALUE_MAX,
};

/// `POST /tags/{resourceArn}` with `tags`.
async fn tag_resource(dsql: &Dsql, arn: &str, tags: &BTreeMap<String, String>) -> Answer {
    dsql.tag_resource(arn, json!({"tags": tag_object(tags)}))
        .await
}

/// `GET /tags/{resourceArn}`, the other reader of the persisted tag map.
async fn list_tags(dsql: &Dsql, arn: &str) -> Answer {
    dsql.list_tags_for_resource(arn).await
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

/// `count` distinct tags under `prefix`, so two calls can ask for disjoint sets.
fn numbered_tags(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|n| (format!("{prefix}-{n}"), format!("value-{n}")))
        .collect()
}

/// The tag map an answer reports, as an absent map and an empty one are the same tag
/// set (`GetCluster` omits the member for an untagged cluster).
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

/// Create a cluster carrying `initial`, returning its identifier and ARN.
async fn cluster_with_tags(dsql: &Dsql, initial: &BTreeMap<String, String>) -> (String, String) {
    let created = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "tags": tag_object(initial),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must answer 200, got {} {:?}",
        created.status, created.body
    );
    (created.str("identifier"), created.str("arn"))
}

/// One `TagResource` call: the tags the resource was created with, the tags the call
/// carries, and the tag set the resource must hold afterwards.
struct MergeCase {
    name: String,
    region: &'static str,
    initial: BTreeMap<String, String>,
    request: BTreeMap<String, String>,
    expected: BTreeMap<String, String>,
}

fn merge_cases() -> Vec<MergeCase> {
    let long_key = "k".repeat(TAG_KEY_MAX);
    let long_value = "v".repeat(TAG_VALUE_MAX);
    // Every character class `TagKey`'s pattern allows, in one key.
    let every_class_key = "aZ9_.:/=+-@ x";

    let mut cases = vec![
        MergeCase {
            name: "an untagged resource takes the one tag the call carries".to_string(),
            region: "us-east-1",
            initial: BTreeMap::new(),
            request: tags(&[("env", "test")]),
            expected: tags(&[("env", "test")]),
        },
        MergeCase {
            name: "a key the resource does not hold is added, the others kept".to_string(),
            region: "eu-west-1",
            initial: tags(&[("env", "prod")]),
            request: tags(&[("owner", "team-a")]),
            expected: tags(&[("env", "prod"), ("owner", "team-a")]),
        },
        MergeCase {
            name: "a key the resource already holds takes the new value".to_string(),
            region: "ap-southeast-2",
            initial: tags(&[("env", "prod"), ("owner", "team-a")]),
            request: tags(&[("env", "staging")]),
            expected: tags(&[("env", "staging"), ("owner", "team-a")]),
        },
        MergeCase {
            name: "overwrite and add in one call".to_string(),
            region: "us-west-2",
            initial: tags(&[("env", "prod"), ("owner", "team-a")]),
            request: tags(&[("env", "dev"), ("tier", "1")]),
            expected: tags(&[("env", "dev"), ("owner", "team-a"), ("tier", "1")]),
        },
        MergeCase {
            name: "an empty tag map leaves the resource as it was".to_string(),
            region: "us-east-2",
            initial: tags(&[("env", "prod")]),
            request: BTreeMap::new(),
            expected: tags(&[("env", "prod")]),
        },
        MergeCase {
            name: "an empty value is a value: TagValue admits length 0".to_string(),
            region: "ca-central-1",
            initial: BTreeMap::new(),
            request: tags(&[("marker", "")]),
            expected: tags(&[("marker", "")]),
        },
        MergeCase {
            name: "a key using every character class TagKey's pattern allows".to_string(),
            region: "eu-central-1",
            initial: BTreeMap::new(),
            request: tags(&[(every_class_key, every_class_key)]),
            expected: tags(&[(every_class_key, every_class_key)]),
        },
    ];

    // The published length bounds of the key and the value.
    cases.push(MergeCase {
        name: format!("a key at TagKey's {TAG_KEY_MAX}-character maximum and a value at TagValue's {TAG_VALUE_MAX}"),
        region: "us-west-1",
        initial: tags(&[("env", "prod")]),
        request: BTreeMap::from([(long_key.clone(), long_value.clone())]),
        expected: BTreeMap::from([
            ("env".to_string(), "prod".to_string()),
            (long_key, long_value),
        ]),
    });

    // Count: many tags at once, over a prior set of a different size.
    let initial = numbered_tags("kept", 10);
    let request = numbered_tags("added", 40);
    let mut expected = initial.clone();
    expected.extend(request.clone());
    cases.push(MergeCase {
        name: "forty tags added to the ten the resource already held".to_string(),
        region: "us-east-1",
        initial,
        request,
        expected,
    });

    cases
}

/// RULE: `TagResource` merges the pairs it carries into the named resource's tag set —
/// overwriting the keys it names, leaving the keys it does not — and persists it where
/// every reader of the resource sees it.
#[tokio::test]
async fn tag_merges_the_requested_pairs_into_the_persisted_tag_set() {
    for case in merge_cases() {
        let dsql = Dsql::in_region(case.region);
        let (identifier, arn) = cluster_with_tags(&dsql, &case.initial).await;
        assert_eq!(
            arn,
            expected_arn(case.region, &identifier),
            "case {:?}: CreateCluster must answer the cluster's own ARN, got {arn:?}",
            case.name
        );

        let tagged = tag_resource(&dsql, &arn, &case.request).await;
        assert_eq!(
            tagged.status, 200,
            "case {:?}: TagResource must answer 200, got {} {:?}",
            case.name, tagged.status, tagged.body
        );

        // Both implemented readers of the persisted map must report the merge.
        let fetched = dsql.get(&identifier).await;
        assert_eq!(
            reported_tags(&fetched),
            case.expected,
            "case {:?}: expected GetCluster to report {:?}, got {:?}",
            case.name,
            case.expected,
            reported_tags(&fetched)
        );
        let listed = list_tags(&dsql, &arn).await;
        assert_eq!(
            reported_tags(&listed),
            case.expected,
            "case {:?}: expected ListTagsForResource to report {:?}, got {:?}",
            case.name,
            case.expected,
            reported_tags(&listed)
        );

        // `@idempotent`: repeating the same call leaves the same tag set.
        let again = tag_resource(&dsql, &arn, &case.request).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: a repeated TagResource must answer 200, got {} {:?}",
            case.name, again.status, again.body
        );
        let after_repeat = dsql.get(&identifier).await;
        assert_eq!(
            reported_tags(&after_repeat),
            case.expected,
            "case {:?}: a repeated call must leave {:?}, got {:?}",
            case.name,
            case.expected,
            reported_tags(&after_repeat)
        );
    }
}

/// RULE: tags are resource metadata, not cluster configuration, so tagging is accepted
/// whenever the resource exists — including while the clock still owes the cluster its
/// `ACTIVE` status — and moves no status.
#[tokio::test]
async fn tagging_is_accepted_before_the_clock_settles_and_changes_no_status() {
    let case = "tag while the cluster is still CREATING";
    let dsql = Dsql::new();
    let (identifier, arn) = cluster_with_tags(&dsql, &BTreeMap::new()).await;

    let tagged = tag_resource(&dsql, &arn, &tags(&[("phase", "creating")])).await;
    assert_eq!(
        tagged.status, 200,
        "case {case:?}: TagResource must answer 200, got {} {:?}",
        tagged.status, tagged.body
    );

    let during = dsql.get(&identifier).await;
    assert_eq!(
        during.str("status"),
        "CREATING",
        "case {case:?}: tagging must not move the status, got {:?}",
        during.str("status")
    );
    assert_eq!(
        reported_tags(&during),
        tags(&[("phase", "creating")]),
        "case {case:?}: expected the tag to be persisted, got {:?}",
        reported_tags(&during)
    );

    wait_for_the_clock().await;
    let settled = dsql.get(&identifier).await;
    assert_eq!(
        settled.str("status"),
        "ACTIVE",
        "case {case:?}: the clock still owes ACTIVE, got {:?}",
        settled.str("status")
    );
    assert_eq!(
        reported_tags(&settled),
        tags(&[("phase", "creating")]),
        "case {case:?}: the tag must survive the settle, got {:?}",
        reported_tags(&settled)
    );
}

/// RULE: a well-formed Aurora DSQL ARN that names no resource this service holds is
/// refused with the ARN-addressed tagging surface's own 404 `NotFoundException` and
/// its fixed sentence, and nothing is written.
///
/// Which resources that covers, and that the sentence names none of them, is the
/// subject of `tests/all/fixed_refusal_sentences.rs`; this row set is about the
/// write leaving the store untouched.
#[tokio::test]
async fn tagging_an_arn_that_names_no_stored_resource_is_not_found() {
    let dsql = Dsql::new();
    // A cluster that DOES exist, so a 404 is about the ARN and not about the store.
    let (live, live_arn) = cluster_with_tags(&dsql, &tags(&[("env", "prod")])).await;

    // A cluster whose deletion the clock has already completed.
    let retired = dsql.create_settled().await;
    let retired_arn = expected_arn("us-east-1", &retired);
    let deleted = dsql
        .call("DELETE", &format!("/cluster/{retired}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteCluster must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    wait_for_the_clock().await;

    let cases: &[(&str, String)] = &[
        (
            "a cluster id this service never minted",
            expected_arn("us-east-1", &absent_identifier()),
        ),
        ("a retired cluster's ARN", retired_arn),
        (
            "a resource-type segment this operation resolves nothing for",
            format!(
                "arn:aws:dsql:us-east-1:{}:stream/abcdefghijklmnopqrstuvwxyz",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            // The nested form `cluster/{cluster}/stream/{stream}` a stream ARN
            // really takes (`state::stream_arn`) does resolve — see
            // `tagging_resolves_every_tagged_resource_by_its_own_arn` — so this row
            // is about a stream of a LIVE cluster that was never created: the
            // cluster resolves and the stream under it does not.
            "a stream of a live cluster, never created",
            expected_stream_arn("us-east-1", &live, &absent_identifier()),
        ),
        (
            "a stream of a cluster that was never created either",
            expected_stream_arn("us-east-1", &absent_identifier(), &absent_identifier()),
        ),
        (
            "a cluster ARN written with a colon before the id",
            format!(
                "arn:aws:dsql:us-east-1:{}:cluster:{}",
                engine::EMULATOR_ACCOUNT_ID,
                absent_identifier()
            ),
        ),
    ];

    for (name, arn) in cases {
        let refused = tag_resource(&dsql, arn, &tags(&[("env", "dev")])).await;
        assert_eq!(
            refused.status, 404,
            "case {name:?}: TagResource must answer 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: the error code must be NotFoundException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.message(),
            "Resource was not found",
            "case {name:?}: the tagging surface's fixed sentence must state the refusal, \
             got {:?}",
            refused.message()
        );
    }

    let untouched = dsql.get(&live).await;
    assert_eq!(
        reported_tags(&untouched),
        tags(&[("env", "prod")]),
        "case {:?}: a refused call must write nothing, got {:?}",
        "the live cluster after the refusals",
        reported_tags(&untouched)
    );

    // Control: the resource's OWN ARN still tags, so the refusals above are about the
    // ARN naming no resource, not about the route or the body.
    let accepted = tag_resource(&dsql, &live_arn, &tags(&[("env", "dev")])).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: the resource's own ARN must answer 200, got {} {:?}",
        "the live cluster's own ARN", accepted.status, accepted.body
    );
    assert_eq!(
        reported_tags(&dsql.get(&live).await),
        tags(&[("env", "dev")]),
        "case {:?}: expected the merged tag set",
        "the live cluster's own ARN"
    );
}

/// RULE: an ARN that names no Aurora DSQL resource at all is refused as invalid input
/// (400 `ValidationException`) rather than looked up — never with the scaffold's 501.
///
/// The BODY of that refusal — the modelled `@required` `reason`, the `fieldList`
/// naming the member, and the three operations on the tagging path agreeing about
/// all of it — is asserted in `tests/all/tagging_arn_refusal_body.rs`.
#[tokio::test]
async fn tagging_an_arn_that_names_no_dsql_resource_is_refused_as_invalid() {
    let dsql = Dsql::new();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let cases: &[(&str, String)] = &[
        (
            "another service's resource",
            format!("arn:aws:kinesis:us-east-1:{account}:stream/orders"),
        ),
        (
            "an S3 ARN, with its empty region and account fields",
            "arn:aws:s3:::my-bucket/key".to_string(),
        ),
        (
            "a dsql ARN with no resource field",
            format!("arn:aws:dsql:us-east-1:{account}:"),
        ),
        (
            "a dsql ARN whose resource field has no type",
            format!("arn:aws:dsql:us-east-1:{account}:cluster"),
        ),
        (
            "a dsql ARN whose resource field has no id",
            format!("arn:aws:dsql:us-east-1:{account}:cluster/"),
        ),
        ("too few ARN fields", "arn:aws:dsql:us-east-1".to_string()),
        // `Arn`'s own `@pattern("^arn:.+$")`: refused at the boundary, before the
        // handler runs, with the same status and code.
        ("not an ARN at all", "cluster/abc".to_string()),
    ];

    for (name, arn) in cases {
        let refused = tag_resource(&dsql, arn, &tags(&[("env", "dev")])).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: TagResource must answer 400, got {} {:?}",
            refused.status, refused.body
        );
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {name:?}: the error code must be ValidationException, got {:?}",
            refused.error
        );
    }
}

/// RULE: a resource holds at most `TagMap`'s maximum number of tags, so a call that
/// would leave it past that ceiling is refused in full with HTTP 402
/// `ServiceQuotaExceededException`; a call landing exactly on the ceiling, and one that
/// only overwrites keys already there, succeed.
#[tokio::test]
async fn tagging_past_the_per_resource_ceiling_is_refused_in_full() {
    let dsql = Dsql::new();
    let initial = numbered_tags("kept", TAG_MAP_MAX - 50);
    let (identifier, arn) = cluster_with_tags(&dsql, &initial).await;

    // Exactly on the published bound: the resource ends holding TAG_MAP_MAX tags.
    let at_bound = tag_resource(&dsql, &arn, &numbered_tags("added", 50)).await;
    assert_eq!(
        at_bound.status, 200,
        "case {:?}: a call landing on the ceiling must answer 200, got {} {:?}",
        "exactly at the ceiling", at_bound.status, at_bound.body
    );
    let full = reported_tags(&dsql.get(&identifier).await);
    assert_eq!(
        full.len(),
        TAG_MAP_MAX,
        "case {:?}: expected {TAG_MAP_MAX} tags, got {}",
        "exactly at the ceiling",
        full.len()
    );

    // One past it: refused, and nothing of the request is applied.
    let refused = tag_resource(&dsql, &arn, &tags(&[("one-too-many", "x")])).await;
    assert_eq!(
        refused.status, 402,
        "case {:?}: expected HTTP 402, got {} {:?}",
        "one past the ceiling", refused.status, refused.body
    );
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|code| code.contains("ServiceQuotaExceededException")),
        "case {:?}: expected ServiceQuotaExceededException, got {:?}",
        "one past the ceiling",
        refused.error
    );
    assert_eq!(
        refused.str("quotaCode"),
        "tags-per-resource",
        "case {:?}: the refusal must name the quota, got {:?}",
        "one past the ceiling",
        refused.str("quotaCode")
    );
    assert_eq!(
        reported_tags(&dsql.get(&identifier).await),
        full,
        "case {:?}: a refused call must write nothing",
        "one past the ceiling"
    );

    // A full resource still takes an overwrite: it adds no entry.
    let overwrite = BTreeMap::from([("kept-0".to_string(), "rewritten".to_string())]);
    let accepted = tag_resource(&dsql, &arn, &overwrite).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: overwriting a key at the ceiling must answer 200, got {} {:?}",
        "overwrite at the ceiling", accepted.status, accepted.body
    );
    let after_overwrite = reported_tags(&dsql.get(&identifier).await);
    assert_eq!(
        after_overwrite.get("kept-0").map(String::as_str),
        Some("rewritten"),
        "case {:?}: expected the overwritten value, got {:?}",
        "overwrite at the ceiling",
        after_overwrite.get("kept-0")
    );
    assert_eq!(
        after_overwrite.len(),
        TAG_MAP_MAX,
        "case {:?}: an overwrite must not change the count, got {}",
        "overwrite at the ceiling",
        after_overwrite.len()
    );
}

/// A `targetDefinition` naming a Kinesis destination, as `CreateStream` requires.
/// Source: `smithy_query(query="shape", shape="KinesisTargetDefinition")` — a
/// `streamArn` and a `roleArn`.
fn kinesis_target(region: &str) -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:{region}:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// Which tagged resource of this service a case addresses. Source: the model's two
/// resources that carry a `TagMap` — a cluster (`CreateCluster.tags` /
/// `GetCluster.tags`) and a stream of one (`CreateStream.tags` / `GetStream.tags`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Cluster,
    Stream,
}

/// One `TagResource` call against ONE kind of tagged resource, with a resource of the
/// OTHER kind standing beside it so a case also pins that only the addressed one moves.
struct ResourceCase {
    name: String,
    region: &'static str,
    kind: Kind,
    /// The tags the ADDRESSED resource is created with.
    initial: BTreeMap<String, String>,
    request: BTreeMap<String, String>,
    expected: BTreeMap<String, String>,
}

/// The tags the resource a case does NOT address is created with, so an assertion can
/// tell one resource's map from the other's.
fn sibling_tags() -> BTreeMap<String, String> {
    tags(&[("env", "prod"), ("owner", "team-a")])
}

fn resource_cases() -> Vec<ResourceCase> {
    let long_key = "k".repeat(TAG_KEY_MAX);
    let long_value = "v".repeat(TAG_VALUE_MAX);
    // Every character class `TagKey`'s pattern allows, in one key.
    let every_class_key = "aZ9_.:/=+-@ x";

    let mut cases = Vec::new();
    for kind in [Kind::Cluster, Kind::Stream] {
        cases.extend([
            ResourceCase {
                name: format!("{kind:?}: an untagged resource takes the one tag the call carries"),
                region: "us-east-1",
                kind,
                initial: BTreeMap::new(),
                request: tags(&[("env", "test")]),
                expected: tags(&[("env", "test")]),
            },
            ResourceCase {
                name: format!("{kind:?}: a key the resource does not hold is added, the rest kept"),
                region: "eu-west-1",
                kind,
                initial: tags(&[("env", "test"), ("scope", "cdc")]),
                request: tags(&[("owner", "team-b")]),
                expected: tags(&[("env", "test"), ("owner", "team-b"), ("scope", "cdc")]),
            },
            ResourceCase {
                name: format!("{kind:?}: a key the resource already holds takes the new value"),
                region: "ap-southeast-2",
                kind,
                initial: tags(&[("env", "test"), ("scope", "cdc")]),
                request: tags(&[("env", "staging"), ("tier", "1")]),
                expected: tags(&[("env", "staging"), ("scope", "cdc"), ("tier", "1")]),
            },
            ResourceCase {
                name: format!("{kind:?}: an empty tag map leaves the resource as it was"),
                region: "us-east-2",
                kind,
                initial: tags(&[("env", "test")]),
                request: BTreeMap::new(),
                expected: tags(&[("env", "test")]),
            },
            ResourceCase {
                name: format!(
                    "{kind:?}: a key at TagKey's {TAG_KEY_MAX}-character maximum and a value at TagValue's {TAG_VALUE_MAX}"
                ),
                region: "us-west-1",
                kind,
                initial: tags(&[(every_class_key, every_class_key)]),
                request: BTreeMap::from([(long_key.clone(), long_value.clone())]),
                expected: BTreeMap::from([
                    (every_class_key.to_string(), every_class_key.to_string()),
                    (long_key.clone(), long_value.clone()),
                ]),
            },
        ]);

        // Count: forty tags at once, over a prior set of a different size.
        let initial = numbered_tags("kept", 10);
        let request = numbered_tags("added", 40);
        let mut expected = initial.clone();
        expected.extend(request.clone());
        cases.push(ResourceCase {
            name: format!("{kind:?}: forty tags added to the ten the resource already held"),
            region: "us-west-2",
            kind,
            initial,
            request,
            expected,
        });
    }
    cases
}

/// RULE: `TagResource` resolves `resourceArn` through the one resolution every tagging
/// operation of this service uses, so EVERY resource of it that carries tags — a
/// cluster, and a stream nested under a cluster's ARN — is tagged by its own ARN, the
/// merge landing in that resource's OWN tag map and nowhere else; an ARN whose nested
/// resource does not exist stays the modelled 404.
#[tokio::test]
async fn tagging_resolves_every_tagged_resource_by_its_own_arn() {
    // The clock check below is one wall-clock wait, so it runs once per resource kind
    // rather than once per row.
    let mut settle_checked: Vec<Kind> = Vec::new();
    for case in resource_cases() {
        let dsql = Dsql::in_region(case.region);
        let (cluster_initial, stream_initial) = match case.kind {
            Kind::Cluster => (case.initial.clone(), sibling_tags()),
            Kind::Stream => (sibling_tags(), case.initial.clone()),
        };

        let (cluster, cluster_arn) = cluster_with_tags(&dsql, &cluster_initial).await;
        // `CreateStream` refuses a cluster the clock still owes its `ACTIVE` status,
        // so the cluster is settled before the stream beside it is created.
        wait_for_the_clock().await;
        let created = dsql
            .create_stream(
                &cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(case.region),
                    "tags": tag_object(&stream_initial),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateStream must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );
        let stream = created.str("streamIdentifier");
        let stream_arn = created.str("arn");
        assert_eq!(
            stream_arn,
            expected_stream_arn(case.region, &cluster, &stream),
            "case {:?}: CreateStream must answer the stream's nested ARN, got {stream_arn:?}",
            case.name
        );

        let (target_arn, sibling_arn) = match case.kind {
            Kind::Cluster => (cluster_arn.clone(), stream_arn.clone()),
            Kind::Stream => (stream_arn.clone(), cluster_arn.clone()),
        };

        let tagged = tag_resource(&dsql, &target_arn, &case.request).await;
        assert_eq!(
            tagged.status, 200,
            "case {:?}: TagResource on {target_arn:?} must answer 200, got {} {:?}",
            case.name, tagged.status, tagged.body
        );

        // The resource's OWN reader, and the shared tag reader, must both report the
        // merge — a hardcoded answer in one of them cannot satisfy both.
        let own = match case.kind {
            Kind::Cluster => dsql.get(&cluster).await,
            Kind::Stream => dsql.get_stream(&cluster, &stream).await,
        };
        assert_eq!(
            reported_tags(&own),
            case.expected,
            "case {:?}: expected the resource's own reader to report {:?}, got {:?}",
            case.name,
            case.expected,
            reported_tags(&own)
        );
        let listed = list_tags(&dsql, &target_arn).await;
        assert_eq!(
            reported_tags(&listed),
            case.expected,
            "case {:?}: expected ListTagsForResource to report {:?}, got {:?}",
            case.name,
            case.expected,
            reported_tags(&listed)
        );

        // The other resource's map is a DIFFERENT map: tagging one never writes it.
        let sibling = list_tags(&dsql, &sibling_arn).await;
        assert_eq!(
            reported_tags(&sibling),
            sibling_tags(),
            "case {:?}: the resource beside it must keep {:?}, got {:?}",
            case.name,
            sibling_tags(),
            reported_tags(&sibling)
        );

        // `@idempotent`: the same call again leaves the same tag set.
        let again = tag_resource(&dsql, &target_arn, &case.request).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: a repeated TagResource must answer 200, got {} {:?}",
            case.name, again.status, again.body
        );
        let after_repeat = list_tags(&dsql, &target_arn).await;
        assert_eq!(
            reported_tags(&after_repeat),
            case.expected,
            "case {:?}: a repeated call must leave {:?}, got {:?}",
            case.name,
            case.expected,
            reported_tags(&after_repeat)
        );

        // And the tag survives the clock settling the resource's status, so the write
        // landed in the stored resource rather than in anything per-request.
        if !settle_checked.contains(&case.kind) {
            settle_checked.push(case.kind);
            wait_for_the_clock().await;
            let settled = list_tags(&dsql, &target_arn).await;
            assert_eq!(
                reported_tags(&settled),
                case.expected,
                "case {:?}: the merge must survive the settle, got {:?}",
                case.name,
                reported_tags(&settled)
            );
        }
    }
}
