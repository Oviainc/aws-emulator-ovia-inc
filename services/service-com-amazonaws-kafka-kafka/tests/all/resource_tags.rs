// SPDX-License-Identifier: CC0-1.0

//! Resource tagging — `TagResource`, `UntagResource` and `ListTagsForResource` —
//! driven end to end through the crate's shared harness (`all/kafka_harness.rs`),
//! so these suites dispatch over exactly the path every other suite here does.
//!
//! RULE under test: the tagging operations treat `ResourceArn` as the identity of a
//! PERSISTED resource. A value that is not an ARN of an MSK resource is refused
//! with the modelled 400 `BadRequestException`; a well-formed MSK resource ARN this
//! endpoint's region holds no resource for is refused with the modelled 404
//! `NotFoundException`; and for a resource that does exist the tag set is a stored
//! map ON that resource — `TagResource` merges the submitted pairs (overwriting a
//! key already present), `UntagResource` removes exactly the keys it names and
//! ignores the ones the resource does not carry, and `ListTagsForResource` reports
//! what is stored, which is the same map every other reader of that resource
//! reports. A tag write is admitted only within the documented limits: a key of
//! 1..128 Unicode characters, no `aws:` prefix and only Unicode letters, digits,
//! white space and `_ . / = + - @` (the model's own `UntagResourceRequest$TagKeys`
//! documentation), a value of at most 256 characters and at most 50 tags on one
//! resource (the published Amazon MSK tag restrictions).
//!
//! The rule is asserted on BOTH resource types this service persists a tag set
//! for — clusters and channels — because the operations resolve any MSK resource
//! ARN, not one kind of it.
//!
//! Each suite asserts that rule over a TABLE, whose rows differ along what the rule
//! generalises over: tag count, key shape, key length at the model's published
//! bounds, value shape, the resource type and partition in the ARN, the region the
//! call is signed for, and the form a malformed ARN takes. The recorded case the
//! issue was filed about (tagging a never-created cluster ARN) is ONE row of the
//! not-found table. Where the rule is an invariant — a tag round-tripping through
//! every reader, a repeated tag or untag changing nothing, a refused request
//! writing nothing — the table walks it directly.

use std::collections::BTreeMap;

use serde_json::{json, Value};
// The handler's own limits, so the suite and the service cannot disagree about
// where a bound is. Their sources are cited on the constants: the key length and
// character rules come from the model's `UntagResourceRequest$TagKeys`
// documentation, the per-resource count and value length from the published Amazon
// MSK tag restrictions.
use service_com_amazonaws_kafka_kafka::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LENGTH, MAX_TAG_VALUE_LENGTH,
};

// The crate's shared test harness: the router, the HTTP call path and the
// `CreateCluster` request builder every suite here dispatches through.
use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// The text the tagging operations answer a resource they cannot find with.
/// RECORDED behaviour, mirroring `state::TAGGING_NOT_FOUND_MESSAGE`: the model
/// gives `NotFoundException` only a free-form `message`, so what this suite pins
/// as a requirement is the 404 and the `NotFoundException` code; the text is
/// asserted so the three operations cannot drift apart from each other.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The shared harness plus the calls this suite's rule needs: the three tagging
/// operations and the creates that seed a resource to tag. Everything else —
/// router, SigV4-scoped request, response parsing, `DescribeCluster`, `pct` — is
/// the harness's.
struct Tagging {
    emulator: Emulator,
}

impl Tagging {
    fn new() -> Self {
        Self {
            emulator: Emulator::new(),
        }
    }

    /// `CreateCluster` with only the required members, plus `tags` when the row
    /// carries any.
    async fn create_cluster(&self, name: &str, tags: &[(&str, &str)]) -> Reply {
        self.create_cluster_with(name, &owned(tags)).await
    }

    /// The same create, for a row whose tag set is built at run time — the
    /// harness's `ClusterRequest` carries `'static` tag pairs, and the bound rows
    /// below build their keys as the suite runs.
    async fn create_cluster_with(&self, name: &str, tags: &[(String, String)]) -> Reply {
        let mut body = ClusterRequest::named(name).to_json();
        if !tags.is_empty() {
            body["tags"] = Value::Object(
                tags.iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            );
        }
        self.emulator.call("POST", "/v1/clusters", Some(body)).await
    }

    /// Create a cluster and hand back the minted ARN.
    async fn cluster_arn(&self, name: &str, tags: &[(&str, &str)]) -> String {
        let reply = self.create_cluster(name, tags).await;
        assert_eq!(
            reply.status, 200,
            "case {name}: expected CreateCluster to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: CreateCluster returned no clusterArn"))
            .to_string()
    }

    async fn tag(&self, arn: &str, pairs: &[(&str, &str)]) -> Reply {
        self.tag_in(REGION, arn, pairs).await
    }

    async fn tag_in(&self, region: &str, arn: &str, pairs: &[(&str, &str)]) -> Reply {
        self.tag_with_in(region, arn, &owned(pairs)).await
    }

    /// `TagResource` with a tag set built at run time.
    async fn tag_with(&self, arn: &str, pairs: &[(String, String)]) -> Reply {
        self.tag_with_in(REGION, arn, pairs).await
    }

    async fn tag_with_in(&self, region: &str, arn: &str, pairs: &[(String, String)]) -> Reply {
        let body = json!({
            "tags": Value::Object(
                pairs
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            ),
        });
        self.emulator
            .call_in(region, "POST", &format!("/v1/tags/{}", pct(arn)), Some(body))
            .await
    }

    async fn untag(&self, arn: &str, keys: &[&str]) -> Reply {
        self.untag_in(REGION, arn, keys).await
    }

    /// `UntagResource` binds `TagKeys` to the `tagKeys` query parameter, which a
    /// list repeats once per key.
    async fn untag_in(&self, region: &str, arn: &str, keys: &[&str]) -> Reply {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", pct(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.emulator
            .call_in(
                region,
                "DELETE",
                &format!("/v1/tags/{}?{query}", pct(arn)),
                None,
            )
            .await
    }

    async fn list_tags(&self, arn: &str) -> Reply {
        self.list_tags_in(REGION, arn).await
    }

    async fn list_tags_in(&self, region: &str, arn: &str) -> Reply {
        self.emulator
            .call_in(region, "GET", &format!("/v1/tags/{}", pct(arn)), None)
            .await
    }

    /// `DeleteCluster`, so a row can ask what the tagging operations answer for a
    /// resource that existed and is gone.
    async fn delete_cluster(&self, arn: &str) -> Reply {
        self.emulator
            .call("DELETE", &format!("/v1/clusters/{}", pct(arn)), None)
            .await
    }

    /// `CreateChannel` under `cluster_arn`, with only the required members plus
    /// the tags the row carries — the second kind of taggable resource this
    /// service persists, so the tagging rule is exercised on more than clusters.
    /// The destination and converter values are the ones
    /// `tests/all/channel_lifecycle.rs` drives `CreateChannel` with.
    async fn create_channel(&self, cluster_arn: &str, name: &str, tags: &[(&str, &str)]) -> Reply {
        let mut body = json!({
            "channelName": name,
            "topicConfigurationList": [{
                "recordConverter": { "valueConverter": "JSON" },
                "topicArn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:topic/channels/orders"),
            }],
            "s3DestinationConfiguration": {
                "deadLetterQueueS3": { "bucketArn": "arn:aws:s3:::msk-channel-dlq" },
                "serviceExecutionRoleArn":
                    "arn:aws:iam::000000000000:role/service-role/msk-channel",
                "storage": {
                    "bucketArn": "arn:aws:s3:::msk-channel-records",
                    "compressionType": "GZIP",
                    "storageClass": "STANDARD",
                },
            },
        });
        if !tags.is_empty() {
            body["tags"] = Value::Object(
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        self.emulator
            .call(
                "POST",
                &format!("/v1/clusters/{}/channels", pct(cluster_arn)),
                Some(body),
            )
            .await
    }

    /// `DescribeChannel`, the channel's own reader — a channel is addressed under
    /// the cluster that holds it.
    async fn describe_channel(&self, cluster_arn: &str, channel_arn: &str) -> Reply {
        self.emulator
            .call(
                "GET",
                &format!(
                    "/v1/clusters/{}/channels/{}",
                    pct(cluster_arn),
                    pct(channel_arn)
                ),
                None,
            )
            .await
    }

    /// `DescribeCluster`, the implemented reader a tag must round-trip through.
    async fn describe(&self, arn: &str) -> Reply {
        self.emulator.describe(arn).await
    }

    /// The tag set `ListTagsForResource` reports, asserted to be present.
    async fn stored_tags(&self, case: &str, arn: &str) -> BTreeMap<String, String> {
        let reply = self.list_tags(arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListTagsForResource to return 200, got {} {}",
            reply.status, reply.body
        );
        tag_map(&reply, "/tags").unwrap_or_else(|| {
            panic!(
                "case {case}: ListTagsForResource answered without a tags member: {}",
                reply.body
            )
        })
    }
}

/// The error envelope's `message`, or `""` when there is none.
fn message(reply: &Reply) -> &str {
    reply.at("/message").as_str().unwrap_or_default()
}

/// The tag map at `pointer`, ordered so a comparison does not depend on the JSON
/// object's iteration order. `None` when the member is absent, which is a
/// different answer from an empty map.
fn tag_map(reply: &Reply, pointer: &str) -> Option<BTreeMap<String, String>> {
    reply.at(pointer).as_object().map(|map| {
        map.iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect()
    })
}

/// `pairs` as the ordered map a reader must report.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Borrowed pairs as owned ones, so one request builder serves both the literal
/// tables and the sets the bound rows build at run time.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The ordered map `pairs` must read back as.
fn expected_owned(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// A tag key of exactly `length` Unicode characters, for the bound rows.
fn key_of_length(length: usize) -> String {
    "k".repeat(length)
}

/// `count` tags whose keys differ only in their ordinal, starting at `first` — the
/// set the per-resource ceiling rows are built from.
fn numbered_tags(first: usize, count: usize) -> Vec<(String, String)> {
    (first..first + count)
        .map(|index| (format!("bulk-{index:04}"), format!("value-{index}")))
        .collect()
}

/// One row of the round-trip table: a name for the case and the tag set it writes.
/// The pairs are owned, so a row can hold a key built at run time (the length
/// bounds) beside the literal ones.
struct TagSet {
    case: &'static str,
    pairs: Vec<(String, String)>,
}

/// One row of the create-then-tag table: the tags the create carried and the ones a
/// later `TagResource` adds to them.
struct MergeCase {
    case: &'static str,
    created: Vec<(&'static str, &'static str)>,
    added: Vec<(&'static str, &'static str)>,
}

/// The same row shape for the channel table, whose sets are literals.
struct ChannelCase {
    case: &'static str,
    created: &'static [(&'static str, &'static str)],
    added: &'static [(&'static str, &'static str)],
}

/// RULE: a tag written on a resource is stored ON that resource — every reader
/// reports the same map, a repeated write changes nothing, a key written twice
/// keeps its latest value, and the keys a request does not name are left alone.
#[tokio::test]
async fn tags_written_are_the_tags_every_reader_reports() {
    // The rows differ along tag count (0, 1, 3, 12), key shape (plain, each
    // allowed special character, white space, Unicode letters, digits, a key at
    // the model's 128-character ceiling and one at its 1-character floor) and
    // value shape (plain, empty, Unicode, long, and a value holding characters a
    // KEY may not hold — the model constrains keys only).
    let rows = [
        TagSet {
            case: "empty-set",
            pairs: vec![],
        },
        TagSet {
            case: "single-pair",
            pairs: owned(&[("env", "prod")]),
        },
        TagSet {
            case: "three-pairs",
            pairs: owned(&[("env", "prod"), ("team", "data"), ("owner", "msk")]),
        },
        TagSet {
            case: "allowed-special-characters",
            pairs: owned(&[
                ("under_score", "1"),
                ("dot.key", "2"),
                ("slash/key", "3"),
                ("equals=key", "4"),
                ("plus+key", "5"),
                ("dash-key", "6"),
                ("at@key", "7"),
                ("white space", "8"),
            ]),
        },
        TagSet {
            case: "unicode-and-digits",
            pairs: owned(&[("ümlaut", "wert"), ("日本語", "値"), ("12345", "numeric")]),
        },
        TagSet {
            case: "key-at-both-length-bounds",
            pairs: vec![
                ("k".to_string(), "shortest-key".to_string()),
                (key_of_length(MAX_TAG_KEY_LENGTH), "longest-key".to_string()),
            ],
        },
        TagSet {
            case: "value-shapes",
            pairs: owned(&[
                ("empty-value", ""),
                ("colon-value", "arn:aws:iam::000000000000:role/r"),
                ("unicode-value", "ünïcøde"),
                ("long-value", "v0123456789012345678901234567890123456789"),
            ]),
        },
        TagSet {
            case: "twelve-pairs",
            pairs: owned(&[
                ("k00", "v00"),
                ("k01", "v01"),
                ("k02", "v02"),
                ("k03", "v03"),
                ("k04", "v04"),
                ("k05", "v05"),
                ("k06", "v06"),
                ("k07", "v07"),
                ("k08", "v08"),
                ("k09", "v09"),
                ("k10", "v10"),
                ("k11", "v11"),
            ]),
        },
    ];

    let emulator = Tagging::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let arn = emulator
            .cluster_arn(&format!("tagged-{index}-{case}"), &[])
            .await;

        // A resource that was never tagged reports an EMPTY map, not a 404 and not
        // an absent member.
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            BTreeMap::new(),
            "case {case}: an untagged cluster must report no tags"
        );

        let tagged = emulator.tag_with(&arn, &row.pairs).await;
        assert_eq!(
            (tagged.status, tagged.error_code()),
            (204, ""),
            "case {case}: expected TagResource to answer 204 with no error, got {} {}",
            tagged.status,
            tagged.body
        );
        assert!(
            tagged.body.is_null(),
            "case {case}: a 204 carries no body, got {}",
            tagged.body
        );

        // Every reader of the resource reports the same stored map.
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            expected_owned(&row.pairs),
            "case {case}: ListTagsForResource must report exactly the tags written"
        );
        let described = emulator.describe(&arn).await;
        assert_eq!(
            tag_map(&described, "/clusterInfo/tags"),
            Some(expected_owned(&row.pairs)),
            "case {case}: DescribeCluster must report the same tags, got {}",
            described.body
        );

        // Reading twice changes nothing: the second answer equals the first.
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            expected_owned(&row.pairs),
            "case {case}: ListTagsForResource must not change what a later caller sees"
        );

        // Writing the same set again is idempotent.
        let again = emulator.tag_with(&arn, &row.pairs).await;
        assert_eq!(
            again.status, 204,
            "case {case}: expected a repeated TagResource to answer 204, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            expected_owned(&row.pairs),
            "case {case}: a repeated TagResource must leave the tag set as it was"
        );

        if let Some(first_key) = row.pairs.first().map(|(key, _)| key.as_str()) {
            // A key already in use has its value OVERWRITTEN, and the keys the
            // request does not name are untouched.
            let overwritten = emulator.tag(&arn, &[(first_key, "rewritten")]).await;
            assert_eq!(
                overwritten.status, 204,
                "case {case}: expected the overwrite to answer 204, got {} {}",
                overwritten.status, overwritten.body
            );
            let mut after_overwrite = expected_owned(&row.pairs);
            after_overwrite.insert(first_key.to_string(), "rewritten".to_string());
            assert_eq!(
                emulator.stored_tags(case, &arn).await,
                after_overwrite,
                "case {case}: re-tagging key {first_key:?} must replace only its value"
            );

            // A merge adds the new key and keeps the rest.
            let merged = emulator.tag(&arn, &[("merged", "yes")]).await;
            assert_eq!(
                merged.status, 204,
                "case {case}: expected the merge to answer 204, got {} {}",
                merged.status, merged.body
            );
            let mut after_merge = after_overwrite.clone();
            after_merge.insert("merged".to_string(), "yes".to_string());
            assert_eq!(
                emulator.stored_tags(case, &arn).await,
                after_merge,
                "case {case}: TagResource must merge rather than replace the tag set"
            );

            // `UntagResource` removes exactly the keys it names...
            let removed = emulator.untag(&arn, &[first_key, "merged"]).await;
            assert_eq!(
                (removed.status, removed.error_code()),
                (204, ""),
                "case {case}: expected UntagResource to answer 204, got {} {}",
                removed.status,
                removed.body
            );
            let mut after_untag = after_merge.clone();
            after_untag.remove(first_key);
            after_untag.remove("merged");
            assert_eq!(
                emulator.stored_tags(case, &arn).await,
                after_untag,
                "case {case}: UntagResource must remove only the keys it names"
            );

            // ...and removing them again, or removing a key the resource never
            // carried, changes nothing.
            let repeat = emulator.untag(&arn, &[first_key, "never-set"]).await;
            assert_eq!(
                repeat.status, 204,
                "case {case}: expected a repeated UntagResource to answer 204, got {} {}",
                repeat.status, repeat.body
            );
            assert_eq!(
                emulator.stored_tags(case, &arn).await,
                after_untag,
                "case {case}: removing an absent key must leave the tag set as it was"
            );
        }
    }
}

/// RULE: tags supplied at create time and tags added later are ONE set on the
/// resource — a later `TagResource` merges into what the create stored, and
/// `UntagResource` can remove a key the create wrote.
#[tokio::test]
async fn tags_from_create_and_from_tag_resource_are_one_set() {
    // Rows differ along how many tags the create carried and how the later call
    // overlaps them: disjoint, overlapping, and identical.
    let rows = [
        MergeCase {
            case: "created-untagged",
            created: vec![],
            added: vec![("added", "1")],
        },
        MergeCase {
            case: "disjoint",
            created: vec![("env", "prod")],
            added: vec![("team", "data"), ("owner", "msk")],
        },
        MergeCase {
            case: "overlapping",
            created: vec![("env", "prod"), ("team", "data")],
            added: vec![("env", "staging")],
        },
        MergeCase {
            case: "identical",
            created: vec![("env", "prod")],
            added: vec![("env", "prod")],
        },
    ];

    let emulator = Tagging::new();
    for MergeCase {
        case,
        created,
        added,
    } in &rows
    {
        let arn = emulator.cluster_arn(case, created).await;
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            expected(created),
            "case {case}: ListTagsForResource must report the tags CreateCluster stored"
        );

        let tagged = emulator.tag(&arn, added).await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: expected TagResource to answer 204, got {} {}",
            tagged.status, tagged.body
        );

        let mut union = expected(created);
        for (key, value) in added {
            union.insert((*key).to_string(), (*value).to_string());
        }
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            union,
            "case {case}: the stored set must be the create's tags merged with the later ones"
        );

        // A key the create wrote is removable by `UntagResource`.
        if let Some((key, _)) = created.first().copied() {
            let removed = emulator.untag(&arn, &[key]).await;
            assert_eq!(
                removed.status, 204,
                "case {case}: expected UntagResource to answer 204, got {} {}",
                removed.status, removed.body
            );
            let mut after = union.clone();
            after.remove(key);
            assert_eq!(
                emulator.stored_tags(case, &arn).await,
                after,
                "case {case}: a tag written by the create must be removable"
            );
        }
    }
}

/// RULE: a well-formed MSK resource ARN this endpoint's region holds no resource
/// for is a 404 `NotFoundException` from every tagging operation — never a success
/// and never an empty tag map, which would mask a bad ARN as "no tags yet".
#[tokio::test]
async fn an_arn_naming_no_resource_is_not_found() {
    let emulator = Tagging::new();

    // A live cluster, so the rows below are distinguished by the ARN alone and not
    // by an empty registry.
    let live = emulator.cluster_arn("live-cluster", &[("env", "prod")]).await;

    // A cluster that existed and was deleted: every later reader must answer as if
    // it never existed.
    let deleted = emulator.cluster_arn("deleted-cluster", &[]).await;
    let dropped = emulator.delete_cluster(&deleted).await;
    assert_eq!(
        dropped.status, 200,
        "expected DeleteCluster to answer 200, got {} {}",
        dropped.status, dropped.body
    );

    // The rows differ along the resource TYPE in the ARN (every taggable MSK
    // resource kind, not only the cluster the issue cited), the partition, the
    // cluster id, and how the resource came to be missing (never created, or
    // deleted).
    let rows: Vec<(&str, String)> = vec![
        (
            "never-created-cluster",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-1111-1111-1111-111111111111-1"),
        ),
        (
            "never-created-cluster-other-id",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/abcdef12-3456-7890-abcd-ef1234567890-99"),
        ),
        (
            "configuration",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/absent/22222222-2222-2222-2222-222222222222-2"),
        ),
        (
            "replicator",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/33333333-3333-3333-3333-333333333333-3"),
        ),
        (
            "vpc-connection",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/absent/44444444-4444-4444-4444-444444444444-4"),
        ),
        (
            "cluster-operation",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/absent/55555555-5555-5555-5555-555555555555-5"),
        ),
        (
            "other-partition",
            format!("arn:aws-us-gov:kafka:{REGION}:{ACCOUNT}:cluster/absent/66666666-6666-6666-6666-666666666666-6"),
        ),
        (
            "other-account",
            format!("arn:aws:kafka:{REGION}:210987654321:cluster/absent/77777777-7777-7777-7777-777777777777-7"),
        ),
        ("deleted-cluster", deleted.clone()),
    ];

    for (case, arn) in &rows {
        let tagged = emulator.tag(arn, &[("env", "prod")]).await;
        assert_eq!(
            (tagged.status, tagged.error_code(), message(&tagged)),
            (404, "NotFoundException", NOT_FOUND_MESSAGE),
            "case {case}: expected TagResource to answer 404 NotFoundException, got {} {}",
            tagged.status,
            tagged.body
        );
        let removed = emulator.untag(arn, &["env"]).await;
        assert_eq!(
            (removed.status, removed.error_code(), message(&removed)),
            (404, "NotFoundException", NOT_FOUND_MESSAGE),
            "case {case}: expected UntagResource to answer 404 NotFoundException, got {} {}",
            removed.status,
            removed.body
        );
        let listed = emulator.list_tags(arn).await;
        assert_eq!(
            (listed.status, listed.error_code(), message(&listed)),
            (404, "NotFoundException", NOT_FOUND_MESSAGE),
            "case {case}: expected ListTagsForResource to answer 404 NotFoundException, got {} {}",
            listed.status,
            listed.body
        );
    }

    // The live cluster is untouched by all of that: a refused call writes nothing
    // anywhere.
    assert_eq!(
        emulator.stored_tags("live-cluster", &live).await,
        expected(&[("env", "prod")]),
        "the live cluster's tags must survive every refused call"
    );

    // A regional endpoint serves only its own region: the SAME live ARN resolves to
    // nothing when the call is signed for another region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let tagged = emulator.tag_in(region, &live, &[("env", "staging")]).await;
        assert_eq!(
            (tagged.status, tagged.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected TagResource to answer 404, got {} {}",
            tagged.status,
            tagged.body
        );
        let listed = emulator.list_tags_in(region, &live).await;
        assert_eq!(
            (listed.status, listed.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected ListTagsForResource to answer 404, got {} {}",
            listed.status,
            listed.body
        );
        let removed = emulator.untag_in(region, &live, &["env"]).await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (404, "NotFoundException"),
            "case region={region}: expected UntagResource to answer 404, got {} {}",
            removed.status,
            removed.body
        );
    }
    assert_eq!(
        emulator.stored_tags("live-cluster", &live).await,
        expected(&[("env", "prod")]),
        "a call signed for another region must not touch this region's cluster"
    );
}

/// RULE: a `ResourceArn` that is not an ARN of an MSK resource is a 400
/// `BadRequestException` — the request the service cannot parse, as distinct from
/// the resource it cannot find.
#[tokio::test]
async fn a_value_that_is_not_an_msk_resource_arn_is_a_bad_request() {
    // The rows differ along HOW the value fails to be an MSK resource ARN: not an
    // ARN at all, too few ARN fields, another service's namespace, an empty
    // required field, no resource type, no resource id, and a cluster ARN whose id
    // path has the wrong number of segments.
    let rows: Vec<(&str, String)> = vec![
        ("not-an-arn", "not-an-arn-at-all".to_string()),
        ("bare-word", "cluster".to_string()),
        (
            "too-few-fields",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}"),
        ),
        (
            "another-service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:queue/absent/1"),
        ),
        (
            "empty-partition",
            format!("arn::kafka:{REGION}:{ACCOUNT}:cluster/absent/1"),
        ),
        (
            "empty-region",
            format!("arn:aws:kafka::{ACCOUNT}:cluster/absent/1"),
        ),
        (
            "empty-account",
            format!("arn:aws:kafka:{REGION}::cluster/absent/1"),
        ),
        (
            "no-resource-type",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:/absent/1"),
        ),
        (
            "no-resource-id",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster"),
        ),
        (
            "empty-resource-id",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/"),
        ),
        (
            "cluster-without-id-segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/only-a-name"),
        ),
        (
            "cluster-with-extra-segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/name/id/extra"),
        ),
    ];

    let emulator = Tagging::new();
    for (case, arn) in &rows {
        let tagged = emulator.tag(arn, &[("env", "prod")]).await;
        assert_eq!(
            (tagged.status, tagged.error_code()),
            (400, "BadRequestException"),
            "case {case} ({arn:?}): expected TagResource to answer 400 BadRequestException, got {} {}",
            tagged.status,
            tagged.body
        );
        assert!(
            message(&tagged).contains(arn.as_str()),
            "case {case}: the refusal must name the value it refused, got {:?}",
            message(&tagged)
        );
        let removed = emulator.untag(arn, &["env"]).await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (400, "BadRequestException"),
            "case {case} ({arn:?}): expected UntagResource to answer 400 BadRequestException, got {} {}",
            removed.status,
            removed.body
        );
        let listed = emulator.list_tags(arn).await;
        assert_eq!(
            (listed.status, listed.error_code()),
            (400, "BadRequestException"),
            "case {case} ({arn:?}): expected ListTagsForResource to answer 400 BadRequestException, got {} {}",
            listed.status,
            listed.body
        );
    }
}

/// RULE: a tag key is held to the rules the model states for it — 1..128 Unicode
/// characters, no `aws:` prefix, and only Unicode letters, digits, white space and
/// `_ . / = + - @`. A key outside them is a 400 `BadRequestException` from every
/// operation that takes tag keys, and a refused request writes NOTHING.
#[tokio::test]
async fn tag_keys_are_held_to_the_rules_the_model_states() {
    let at_bound = key_of_length(MAX_TAG_KEY_LENGTH);
    let past_bound = key_of_length(MAX_TAG_KEY_LENGTH + 1);

    // Accepted: one row per character class the model's documentation lists, plus
    // each published length bound (1 and 128 characters).
    let accepted: Vec<(&str, String)> = vec![
        ("letters", "release".to_string()),
        ("digits", "12345".to_string()),
        ("unicode-letters", "日本語".to_string()),
        ("white-space", "two words".to_string()),
        ("underscore", "a_b".to_string()),
        ("dot", "a.b".to_string()),
        ("slash", "a/b".to_string()),
        ("equals", "a=b".to_string()),
        ("plus", "a+b".to_string()),
        ("dash", "a-b".to_string()),
        ("at", "a@b".to_string()),
        ("one-character", "k".to_string()),
        ("at-length-bound", at_bound),
        // `aws` is only reserved as the prefix `aws:`; the bare word is a legal key.
        ("reserved-word-without-colon", "aws".to_string()),
    ];

    // Refused: one row per rule, each just outside it.
    let refused: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("past-length-bound", past_bound),
        ("reserved-prefix", "aws:cloudformation".to_string()),
        ("colon", "team:data".to_string()),
        ("hash", "bad#key".to_string()),
        ("comma", "a,b".to_string()),
        ("percent", "a%b".to_string()),
        ("ampersand", "a&b".to_string()),
        ("quote", "a\"b".to_string()),
        ("backslash", "a\\b".to_string()),
        ("parenthesis", "a(b)".to_string()),
    ];

    let emulator = Tagging::new();
    let arn = emulator.cluster_arn("tag-key-rules", &[("env", "prod")]).await;
    let mut stored = expected(&[("env", "prod")]);

    for (case, key) in &accepted {
        let tagged = emulator.tag(&arn, &[(key.as_str(), "accepted")]).await;
        assert_eq!(
            (tagged.status, tagged.error_code()),
            (204, ""),
            "case {case} ({key:?}): expected TagResource to accept the key, got {} {}",
            tagged.status,
            tagged.body
        );
        stored.insert(key.clone(), "accepted".to_string());
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            stored,
            "case {case}: an accepted key must be stored"
        );

        let removed = emulator.untag(&arn, &[key.as_str()]).await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (204, ""),
            "case {case} ({key:?}): expected UntagResource to accept the key, got {} {}",
            removed.status,
            removed.body
        );
        stored.remove(key.as_str());
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            stored,
            "case {case}: an accepted key must be removable"
        );
    }

    for (case, key) in &refused {
        // Sent together with a perfectly good pair, so the assertion below shows
        // that a refused request writes NOTHING rather than writing what it could.
        let tagged = emulator
            .tag(&arn, &[(key.as_str(), "refused"), ("good", "pair")])
            .await;
        assert_eq!(
            (tagged.status, tagged.error_code()),
            (400, "BadRequestException"),
            "case {case} ({key:?}): expected TagResource to refuse the key, got {} {}",
            tagged.status,
            tagged.body
        );
        assert_eq!(
            emulator.stored_tags(case, &arn).await,
            stored,
            "case {case}: a refused TagResource must write nothing at all"
        );

        let removed = emulator.untag(&arn, &[key.as_str()]).await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (400, "BadRequestException"),
            "case {case} ({key:?}): expected UntagResource to refuse the key, got {} {}",
            removed.status,
            removed.body
        );

        // The same rules hold where a create supplies the keys, and a refused
        // create registers no cluster: the name is still free afterwards.
        let name = format!("create-refused-{case}");
        let created = emulator
            .create_cluster(&name, &[(key.as_str(), "refused")])
            .await;
        assert_eq!(
            (created.status, created.error_code()),
            (400, "BadRequestException"),
            "case {case} ({key:?}): expected CreateCluster to refuse the key, got {} {}",
            created.status,
            created.body
        );
        let retried = emulator.create_cluster(&name, &[("env", "prod")]).await;
        assert_eq!(
            retried.status, 200,
            "case {case}: a refused create must register nothing, so the name stays free; got {} {}",
            retried.status, retried.body
        );
    }

    // The tag set the suite started with is exactly what it ends with.
    assert_eq!(
        emulator.stored_tags("final", &arn).await,
        expected(&[("env", "prod")]),
        "the cluster's tags must end where they started"
    );
}

/// RULE: a resource carries at most the documented number of tags and tag values
/// at most the documented length. The ceiling is on the RESULTING set, so it holds
/// across a run of calls and not only within one, and only a key the resource does
/// not already carry counts towards it.
#[tokio::test]
async fn the_tag_count_and_value_length_a_resource_accepts_are_bounded() {
    let emulator = Tagging::new();

    // Exactly at the published ceiling, in one call: accepted.
    let arn = emulator.cluster_arn("at-the-ceiling", &[]).await;
    let full = numbered_tags(0, MAX_TAGS_PER_RESOURCE);
    let tagged = emulator.tag_with(&arn, &full).await;
    assert_eq!(
        (tagged.status, tagged.error_code()),
        (204, ""),
        "case at-the-ceiling: expected {MAX_TAGS_PER_RESOURCE} tags to be accepted, got {} {}",
        tagged.status,
        tagged.body
    );
    let at_ceiling = expected_owned(&full);
    assert_eq!(
        emulator.stored_tags("at-the-ceiling", &arn).await,
        at_ceiling,
        "case at-the-ceiling: the resource must hold exactly the tags written"
    );

    // One more NEW key, in a second call: refused, and nothing is written — the
    // bound is on the resulting set, so a caller cannot walk past it one call at
    // a time.
    let one_past = numbered_tags(MAX_TAGS_PER_RESOURCE, 1);
    let over = emulator.tag_with(&arn, &one_past).await;
    assert_eq!(
        (over.status, over.error_code()),
        (400, "BadRequestException"),
        "case one-past-accumulated: expected a refusal past {MAX_TAGS_PER_RESOURCE} tags, got {} {}",
        over.status,
        over.body
    );
    assert_eq!(
        emulator.stored_tags("one-past-accumulated", &arn).await,
        at_ceiling,
        "case one-past-accumulated: a refused TagResource must write nothing"
    );

    // Re-tagging a key the resource ALREADY carries adds no entry, so it is
    // accepted at the ceiling and only the value changes.
    let rewrite = vec![(full[0].0.clone(), "rewritten".to_string())];
    let rewritten = emulator.tag_with(&arn, &rewrite).await;
    assert_eq!(
        (rewritten.status, rewritten.error_code()),
        (204, ""),
        "case rewrite-at-ceiling: expected an existing key to be re-taggable at the ceiling, got {} {}",
        rewritten.status,
        rewritten.body
    );
    let mut after_rewrite = at_ceiling.clone();
    after_rewrite.insert(full[0].0.clone(), "rewritten".to_string());
    assert_eq!(
        emulator.stored_tags("rewrite-at-ceiling", &arn).await,
        after_rewrite,
        "case rewrite-at-ceiling: only the value may change"
    );

    // Freeing one slot makes room for exactly one new key.
    let removed = emulator.untag(&arn, &[full[0].0.as_str()]).await;
    assert_eq!(
        removed.status, 204,
        "case free-one-slot: expected UntagResource to answer 204, got {} {}",
        removed.status, removed.body
    );
    let refill = emulator.tag_with(&arn, &one_past).await;
    assert_eq!(
        (refill.status, refill.error_code()),
        (204, ""),
        "case free-one-slot: expected the freed slot to accept a new key, got {} {}",
        refill.status,
        refill.body
    );
    let mut after_refill = after_rewrite.clone();
    after_refill.remove(&full[0].0);
    after_refill.insert(one_past[0].0.clone(), one_past[0].1.clone());
    assert_eq!(
        emulator.stored_tags("free-one-slot", &arn).await,
        after_refill,
        "case free-one-slot: the resource must hold the ceiling's worth of tags again"
    );
    assert_eq!(
        after_refill.len(),
        MAX_TAGS_PER_RESOURCE,
        "case free-one-slot: the suite's own bookkeeping must sit at the ceiling, got {}",
        after_refill.len()
    );

    // One call carrying one more than the ceiling: refused, nothing written.
    let fresh = emulator.cluster_arn("one-past-in-one-call", &[]).await;
    let too_many = numbered_tags(0, MAX_TAGS_PER_RESOURCE + 1);
    let refused = emulator.tag_with(&fresh, &too_many).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "BadRequestException"),
        "case one-past-in-one-call: expected a refusal, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        emulator.stored_tags("one-past-in-one-call", &fresh).await,
        BTreeMap::new(),
        "case one-past-in-one-call: a refused TagResource must write nothing"
    );

    // The same bound where a CREATE supplies the tags: at the ceiling it is
    // accepted and reported, one past it the create is refused and registers
    // nothing, so the name stays free.
    let created = emulator.create_cluster_with("create-at-ceiling", &full).await;
    assert_eq!(
        created.status, 200,
        "case create-at-ceiling: expected CreateCluster to accept {MAX_TAGS_PER_RESOURCE} tags, got {} {}",
        created.status, created.body
    );
    let created_arn = created.body["clusterArn"]
        .as_str()
        .expect("case create-at-ceiling: CreateCluster returned no clusterArn")
        .to_string();
    assert_eq!(
        emulator.stored_tags("create-at-ceiling", &created_arn).await,
        at_ceiling,
        "case create-at-ceiling: the create's tags must be reported back"
    );
    let over_create = emulator
        .create_cluster_with("create-one-past", &too_many)
        .await;
    assert_eq!(
        (over_create.status, over_create.error_code()),
        (400, "BadRequestException"),
        "case create-one-past: expected CreateCluster to refuse {} tags, got {} {}",
        too_many.len(),
        over_create.status,
        over_create.body
    );
    let retried = emulator
        .create_cluster("create-one-past", &[("env", "prod")])
        .await;
    assert_eq!(
        retried.status, 200,
        "case create-one-past: a refused create must register nothing, so the name stays free; got {} {}",
        retried.status, retried.body
    );

    // Tag VALUES: each published bound, and one character past it.
    let value_rows = [
        ("empty-value", 0usize, 204u16),
        ("short-value", 1, 204),
        ("at-value-bound", MAX_TAG_VALUE_LENGTH, 204),
        ("one-past-value-bound", MAX_TAG_VALUE_LENGTH + 1, 400),
    ];
    let values_arn = emulator.cluster_arn("value-bounds", &[]).await;
    let mut stored = BTreeMap::new();
    for (case, length, status) in value_rows {
        let pairs = vec![(format!("value-{length}"), "v".repeat(length))];
        let reply = emulator.tag_with(&values_arn, &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected a {status} for a {length}-character value, got {} {}",
            reply.status, reply.body
        );
        if status == 204 {
            stored.insert(pairs[0].0.clone(), pairs[0].1.clone());
        } else {
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case}: expected BadRequestException, got {}",
                reply.body
            );
        }
        assert_eq!(
            emulator.stored_tags(case, &values_arn).await,
            stored,
            "case {case}: the stored set must hold exactly the accepted values"
        );
    }
}

/// RULE, on the OTHER resource type this service persists: the tagging operations
/// resolve any MSK resource ARN, so a channel's tags round-trip exactly as a
/// cluster's do — and a channel ARN whose channel this region does not hold is the
/// same 404 as any other missing resource.
#[tokio::test]
async fn a_channels_tags_round_trip_through_the_same_operations() {
    let emulator = Tagging::new();
    let cluster = emulator.cluster_arn("channel-host", &[]).await;

    // Rows differ along the tag set the create carried and the one a later
    // `TagResource` adds, the same dimensions the cluster rows walk.
    let rows = [
        ChannelCase {
            case: "created-untagged",
            created: &[],
            added: &[("env", "prod")],
        },
        ChannelCase {
            case: "created-tagged-disjoint",
            created: &[("env", "prod")],
            added: &[("team", "data")],
        },
        ChannelCase {
            case: "created-tagged-overlapping",
            created: &[("env", "prod")],
            added: &[("env", "staging"), ("owner", "msk")],
        },
    ];

    for ChannelCase {
        case,
        created,
        added,
    } in rows
    {
        let reply = emulator
            .create_channel(&cluster, &format!("stream-{case}"), created)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected CreateChannel to answer 200, got {} {}",
            reply.status, reply.body
        );
        let channel_arn = reply
            .at("/channelArn")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: CreateChannel returned no channelArn: {}",
                    reply.body
                )
            })
            .to_string();

        assert_eq!(
            emulator.stored_tags(case, &channel_arn).await,
            expected(created),
            "case {case}: ListTagsForResource must report the tags CreateChannel stored"
        );

        let tagged = emulator.tag(&channel_arn, added).await;
        assert_eq!(
            (tagged.status, tagged.error_code()),
            (204, ""),
            "case {case}: expected TagResource on a channel to answer 204, got {} {}",
            tagged.status,
            tagged.body
        );
        let mut union = expected(created);
        for (key, value) in added {
            union.insert((*key).to_string(), (*value).to_string());
        }
        assert_eq!(
            emulator.stored_tags(case, &channel_arn).await,
            union,
            "case {case}: the channel must carry the create's tags merged with the later ones"
        );
        // The channel's own reader reports the same map — tags live on the
        // resource record, not in a store beside it.
        let described = emulator.describe_channel(&cluster, &channel_arn).await;
        assert_eq!(
            tag_map(&described, "/tags"),
            Some(union.clone()),
            "case {case}: DescribeChannel must report the same tags, got {}",
            described.body
        );

        // And a key goes away exactly as it does on a cluster.
        let removed = emulator
            .untag(&channel_arn, &[added[0].0, "never-set"])
            .await;
        assert_eq!(
            (removed.status, removed.error_code()),
            (204, ""),
            "case {case}: expected UntagResource on a channel to answer 204, got {} {}",
            removed.status,
            removed.body
        );
        let mut after = union.clone();
        after.remove(added[0].0);
        assert_eq!(
            emulator.stored_tags(case, &channel_arn).await,
            after,
            "case {case}: UntagResource must remove only the key it named"
        );
    }

    // A well-formed channel ARN this region holds no channel for is the same 404
    // every other missing resource earns; a channel ARN of the wrong SHAPE is the
    // 400 a value that is not an MSK resource ARN earns.
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:channel/channel-host/\
         11111111-1111-1111-1111-111111111111-1/absent/22222222-2222-2222-2222-222222222222-2"
    );
    let reply = emulator.list_tags(&absent).await;
    assert_eq!(
        (reply.status, reply.error_code(), message(&reply)),
        (404, "NotFoundException", NOT_FOUND_MESSAGE),
        "case absent-channel: expected a 404 NotFoundException, got {} {}",
        reply.status,
        reply.body
    );
    let malformed = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:channel/only-a-name");
    let reply = emulator.list_tags(&malformed).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case malformed-channel-arn: expected a 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
}
