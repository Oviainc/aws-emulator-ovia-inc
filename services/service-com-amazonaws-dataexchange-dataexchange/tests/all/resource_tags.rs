// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's tagging surface: `TagResource`,
//! `ListTagsForResource` and `UntagResource`, all three bound to
//! `/tags/{ResourceArn}`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `call`, `create_data_set`, `create_revision`) is the one
//! `data_sets.rs` and `revisions.rs` already use.
//!
//! The tables span the classes the rules generalise over — both resource kinds this
//! service tags, tag maps from empty to the published maximum count, keys and values
//! at the published character bounds and one past them, non-ASCII keys and values,
//! merges that overlap and merges that do not, removals of present, absent and
//! repeated keys, values that are not ARNs at all, and ARNs that address nothing this
//! emulator holds — another vendor's, another partition's, region's or account's, a
//! resource kind nothing is stored of, or an id naming no record.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, emulator, Answer, ACCOUNT, REGION};
use crate::revisions::create_revision;

/// A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) for a resource this emulator does not
/// hold, so an answer about it is about the resource rather than about the request.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// AWS's tagging limits, mirrored by `crate::state::MAX_TAGS`,
/// `crate::state::MAX_TAG_KEY_CHARS` and `crate::state::MAX_TAG_VALUE_CHARS`: at most
/// 50 tags on one resource, a key of up to 128 characters and a value of up to 256.
const MAX_TAGS: usize = 50;
const MAX_TAG_KEY_CHARS: usize = 128;
const MAX_TAG_VALUE_CHARS: usize = 256;

/// The tagging surface's wire member name. `ListTagsForResource`'s output member and
/// `TagResource`'s input member both carry `@jsonName("tags")`
/// (`smithy_query(query="operation", operation="ListTagsForResource,TagResource")`),
/// so the map travels as `tags` there — while a resource's OWN `Tags` member, on
/// `GetDataSet` / `GetRevision`, carries no `@jsonName` and travels as `Tags`.
const TAGS_ON_THE_WIRE: &str = "tags";

/// Percent-encode one `@httpLabel` value the way an SDK does: the whole ARN is ONE
/// path segment, so its `:` and `/` have to travel as `%3A` and `%2F` or the request
/// addresses a different route.
fn percent_encode(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// `GET /tags/{ResourceArn}`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", percent_encode(arn)), None).await
}

/// `POST /tags/{ResourceArn}` with the given map.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", percent_encode(arn)),
        Some(json!({ TAGS_ON_THE_WIRE: tags })),
    )
    .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=..` — `TagKeys` is an `@httpQuery` list, so
/// each key is its own repetition of the parameter.
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect::<Vec<String>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", percent_encode(arn)),
        None,
    )
    .await
}

/// Which kind of resource a case tags. Both kinds are created with a `Tags` member
/// of their own (`CreateDataSet` / `CreateRevision`), so both are taggable per the
/// model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    DataSet,
    Revision,
}

/// Every resource kind this service's tagging surface addresses.
const KINDS: &[Kind] = &[Kind::DataSet, Kind::Revision];

/// A freshly created resource of `kind`: its `Arn`, and the path its own
/// single-resource reader answers on, so a tag write can be read back through BOTH
/// the tagging surface and the resource itself.
struct Resource {
    arn: String,
    read_path: String,
}

async fn fresh_resource(
    router: &MultiServiceRouter,
    kind: Kind,
    name: &str,
    tags: Option<Value>,
) -> Resource {
    match kind {
        Kind::DataSet => {
            let created = create_data_set(router, name, "tagging", "S3_SNAPSHOT", tags).await;
            assert_eq!(
                created.status, 201,
                "case {name}: expected 201 from CreateDataSet, got {} body {:?}",
                created.status, created.body
            );
            Resource {
                arn: created.string("Arn"),
                read_path: format!("/v1/data-sets/{}", created.string("Id")),
            }
        }
        Kind::Revision => {
            let data_set = create_data_set(router, name, "tagging", "S3_SNAPSHOT", None).await;
            assert_eq!(
                data_set.status, 201,
                "case {name}: expected 201 from CreateDataSet, got {} body {:?}",
                data_set.status, data_set.body
            );
            let data_set_id = data_set.string("Id");
            let created = create_revision(router, &data_set_id, None, tags.as_ref(), name).await;
            Resource {
                arn: created.string("Arn"),
                read_path: format!(
                    "/v1/data-sets/{data_set_id}/revisions/{}",
                    created.string("Id")
                ),
            }
        }
    }
}

/// The `Tags` the resource's OWN reader (`GetDataSet` / `GetRevision`) reports: an
/// untagged resource carries no member at all there, which reads as the empty map.
async fn tags_through_the_resource(router: &MultiServiceRouter, read_path: &str) -> Value {
    let answer = call(router, "GET", read_path, None).await;
    assert_eq!(
        answer.status, 200,
        "expected 200 reading {read_path}, got {} body {:?}",
        answer.status, answer.body
    );
    match answer.member("Tags") {
        Value::Null => json!({}),
        tags => tags.clone(),
    }
}

/// One step of a tag-write sequence: either a write or a removal, with the map the
/// resource is expected to carry afterwards.
enum Step {
    Tag(Value, Value),
    Untag(&'static [&'static str], Value),
}

/// One row of the tag-map catalog.
struct TagCase {
    case: &'static str,
    tags: Value,
}

/// A map of `count` distinct tags, for the rows about how many a resource carries.
fn many_tags(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index:03}"), json!(format!("value-{index:03}")));
    }
    Value::Object(map)
}

/// The rows span the tag maps AWS admits: none, one, several, an empty value, keys
/// and values outside ASCII, a key and a value at the published character maxima,
/// and the published maximum number of tags.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no tags",
            tags: json!({}),
        },
        TagCase {
            case: "one tag",
            tags: json!({ "team": "data" }),
        },
        TagCase {
            case: "several tags",
            tags: json!({ "team": "data", "tier": "gold", "owner": "ops" }),
        },
        TagCase {
            case: "a tag with an empty value",
            tags: json!({ "optional": "" }),
        },
        TagCase {
            case: "non-ascii keys and values",
            tags: json!({ "регион": "eu-центр", "環境": "本番" }),
        },
        TagCase {
            case: "a key at the published maximum length",
            tags: json!({ "k".repeat(MAX_TAG_KEY_CHARS): "at-the-key-bound" }),
        },
        TagCase {
            case: "a value at the published maximum length",
            tags: json!({ "long-value": "v".repeat(MAX_TAG_VALUE_CHARS) }),
        },
        TagCase {
            case: "the published maximum number of tags",
            tags: many_tags(MAX_TAGS),
        },
    ]
}

/// RULE: `ListTagsForResource` answers 200 with exactly the tag map persisted on the
/// resource its `ResourceArn` addresses — the map a `TagResource` write left there,
/// and the empty map for a resource carrying none.
#[tokio::test]
async fn tag_then_list_round_trips_every_tag_map_on_every_resource_kind() {
    let router = emulator();

    for kind in KINDS {
        for TagCase { case, tags } in tag_cases() {
            let case = format!("{kind:?}: {case}");
            let resource = fresh_resource(&router, *kind, "tag-roundtrip", None).await;

            // A resource nothing has tagged reports the empty map, not a 404 and not
            // an absent member.
            let before = list_tags(&router, &resource.arn).await;
            assert_eq!(
                before.status, 200,
                "case {case}: expected 200 from ListTagsForResource on an untagged \
                 resource, got {} body {:?}",
                before.status, before.body
            );
            assert_eq!(
                before.member(TAGS_ON_THE_WIRE),
                &json!({}),
                "case {case}: expected an empty tag map before any write, got {:?}",
                before.member(TAGS_ON_THE_WIRE)
            );

            let written = tag_resource(&router, &resource.arn, &tags).await;
            assert_eq!(
                written.status, 204,
                "case {case}: expected 204 from TagResource, got {} body {:?}",
                written.status, written.body
            );

            let listed = list_tags(&router, &resource.arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case}: expected 200 from ListTagsForResource, got {} body {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                listed.member(TAGS_ON_THE_WIRE),
                &tags,
                "case {case}: expected the written tag map, got {:?}",
                listed.member(TAGS_ON_THE_WIRE)
            );

            // And the resource's own reader agrees: one stored map, two readers.
            let through_resource = tags_through_the_resource(&router, &resource.read_path).await;
            assert_eq!(
                through_resource, tags,
                "case {case}: expected the resource's own Tags member to agree with the \
                 tagging surface, got {through_resource:?}"
            );
        }
    }
}

/// RULE: tags given at create time are the resource's tags, so the tagging surface
/// reports them without any `TagResource` call.
#[tokio::test]
async fn tags_set_at_create_time_are_reported_by_the_tagging_surface() {
    let router = emulator();

    for kind in KINDS {
        for TagCase { case, tags } in tag_cases() {
            let case = format!("{kind:?}: {case}");
            // A create cannot carry an empty `Tags` map through this harness as a
            // distinct state — the member is simply omitted — so that row is covered
            // by the untagged assertions above.
            let created_with = (tags != json!({})).then(|| tags.clone());
            let resource =
                fresh_resource(&router, *kind, "created-with-tags", created_with).await;

            let listed = list_tags(&router, &resource.arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case}: expected 200 from ListTagsForResource, got {} body {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                listed.member(TAGS_ON_THE_WIRE),
                &tags,
                "case {case}: expected the tags the create persisted, got {:?}",
                listed.member(TAGS_ON_THE_WIRE)
            );
        }
    }
}

/// RULE: a tag write merges by key — a key the request names takes the request's
/// value and every other key is left alone — and `UntagResource` removes exactly the
/// keys it names, so the stored map after a sequence of writes is the merge of them.
#[tokio::test]
async fn writes_merge_by_key_and_removals_take_only_the_named_keys() {
    let router = emulator();

    for kind in KINDS {
        // The rows differ along how the write overlaps what is stored (disjoint,
        // partly overlapping, a pure overwrite), and along whether a removal names a
        // key that is there, one that is not, or every key at once.
        let steps = vec![
            (
                "first write",
                Step::Tag(
                    json!({ "team": "data", "tier": "gold" }),
                    json!({ "team": "data", "tier": "gold" }),
                ),
            ),
            (
                "a disjoint write keeps what is stored",
                Step::Tag(
                    json!({ "owner": "ops" }),
                    json!({ "team": "data", "tier": "gold", "owner": "ops" }),
                ),
            ),
            (
                "an overlapping write replaces only the named keys",
                Step::Tag(
                    json!({ "tier": "silver", "stage": "beta" }),
                    json!({ "team": "data", "tier": "silver", "owner": "ops", "stage": "beta" }),
                ),
            ),
            (
                "an empty write changes nothing",
                Step::Tag(
                    json!({}),
                    json!({ "team": "data", "tier": "silver", "owner": "ops", "stage": "beta" }),
                ),
            ),
            (
                "a removal takes only the named key",
                Step::Untag(
                    &["tier"],
                    json!({ "team": "data", "owner": "ops", "stage": "beta" }),
                ),
            ),
            (
                "removing the same key again is idempotent",
                Step::Untag(
                    &["tier"],
                    json!({ "team": "data", "owner": "ops", "stage": "beta" }),
                ),
            ),
            (
                "a removal of a key that was never set changes nothing",
                Step::Untag(
                    &["never-set"],
                    json!({ "team": "data", "owner": "ops", "stage": "beta" }),
                ),
            ),
            (
                "several keys at once, one of them absent",
                Step::Untag(&["team", "stage", "absent"], json!({ "owner": "ops" })),
            ),
            ("the last key", Step::Untag(&["owner"], json!({}))),
        ];

        let resource = fresh_resource(&router, *kind, "tag-merge", None).await;
        for (step, action) in steps {
            let case = format!("{kind:?}: {step}");
            let expected = match action {
                Step::Tag(tags, expected) => {
                    let written = tag_resource(&router, &resource.arn, &tags).await;
                    assert_eq!(
                        written.status, 204,
                        "case {case}: expected 204 from TagResource, got {} body {:?}",
                        written.status, written.body
                    );
                    expected
                }
                Step::Untag(keys, expected) => {
                    let removed = untag_resource(&router, &resource.arn, keys).await;
                    assert_eq!(
                        removed.status, 204,
                        "case {case}: expected 204 from UntagResource, got {} body {:?}",
                        removed.status, removed.body
                    );
                    expected
                }
            };

            let listed = list_tags(&router, &resource.arn).await;
            assert_eq!(
                listed.member(TAGS_ON_THE_WIRE),
                &expected,
                "case {case}: expected the merged tag map, got {:?}",
                listed.member(TAGS_ON_THE_WIRE)
            );
            let through_resource = tags_through_the_resource(&router, &resource.read_path).await;
            assert_eq!(
                through_resource, expected,
                "case {case}: expected the resource's own Tags member to agree, got \
                 {through_resource:?}"
            );
        }
    }
}

/// RULE: `ListTagsForResource` is a read — repeating it answers the identical map and
/// leaves what every other reader sees untouched.
#[tokio::test]
async fn listing_tags_changes_nothing() {
    let router = emulator();
    let resource = fresh_resource(&router, Kind::DataSet, "stable-tags", None).await;
    let tags = json!({ "team": "data", "tier": "gold" });
    let written = tag_resource(&router, &resource.arn, &tags).await;
    assert_eq!(
        written.status, 204,
        "case setup: expected 204 from TagResource, got {} body {:?}",
        written.status, written.body
    );

    let first = list_tags(&router, &resource.arn).await;
    // The rows differ along how many reads preceded this one: a read that moved
    // state would show up as a difference on any of them.
    for round in 1..=3 {
        let case = format!("read {round}");
        let again = list_tags(&router, &resource.arn).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected 200, got {} body {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "case {case}: expected the identical map on every read, got {:?} for {:?}",
            again.body, first.body
        );
    }
    let through_resource = tags_through_the_resource(&router, &resource.read_path).await;
    assert_eq!(
        through_resource, tags,
        "case after three reads: expected the resource's Tags untouched by reads, got \
         {through_resource:?}"
    );
}

/// RULE: a resource carries at most the published number of tags, each with a key and
/// a value no longer than the published maxima; a write that would leave more than
/// that — counting what is already stored — is a 400 `ValidationException` that
/// stores nothing.
#[tokio::test]
async fn a_write_past_a_published_tagging_bound_is_refused_and_stores_nothing() {
    let router = emulator();

    // Rows differ along WHICH bound the write breaks, and sit at the bound as well as
    // one past it.
    let rows: Vec<(String, Value, u16)> = vec![
        (
            "the maximum number of tags".to_string(),
            many_tags(MAX_TAGS),
            204,
        ),
        (
            "one tag past the maximum".to_string(),
            many_tags(MAX_TAGS + 1),
            400,
        ),
        (
            "a key at the maximum length".to_string(),
            json!({ "k".repeat(MAX_TAG_KEY_CHARS): "v" }),
            204,
        ),
        (
            "a key one character past the maximum".to_string(),
            json!({ "k".repeat(MAX_TAG_KEY_CHARS + 1): "v" }),
            400,
        ),
        (
            "a value at the maximum length".to_string(),
            json!({ "k": "v".repeat(MAX_TAG_VALUE_CHARS) }),
            204,
        ),
        (
            "a value one character past the maximum".to_string(),
            json!({ "k": "v".repeat(MAX_TAG_VALUE_CHARS + 1) }),
            400,
        ),
    ];

    for (case, tags, expected) in &rows {
        let resource = fresh_resource(&router, Kind::DataSet, "tag-bounds", None).await;
        let answer = tag_resource(&router, &resource.arn, tags).await;
        assert_eq!(
            answer.status, *expected,
            "case {case}: expected {expected}, got {} body {:?}",
            answer.status, answer.body
        );
        if *expected == 400 {
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {:?}",
                answer.error
            );
            let listed = list_tags(&router, &resource.arn).await;
            assert_eq!(
                listed.member(TAGS_ON_THE_WIRE),
                &json!({}),
                "case {case}: expected a refused write to store nothing, got {:?}",
                listed.member(TAGS_ON_THE_WIRE)
            );
        }
    }

    // And the count bound counts what is ALREADY stored: a merge that would take the
    // resource past it is refused, and the stored map is left exactly as it was.
    let resource = fresh_resource(&router, Kind::DataSet, "tag-merge-bound", None).await;
    let filled = many_tags(MAX_TAGS);
    let written = tag_resource(&router, &resource.arn, &filled).await;
    assert_eq!(
        written.status, 204,
        "case merge setup: expected 204, got {} body {:?}",
        written.status, written.body
    );
    let over = tag_resource(&router, &resource.arn, &json!({ "one-too-many": "x" })).await;
    assert_eq!(
        over.status, 400,
        "case merge past the maximum: expected 400, got {} body {:?}",
        over.status, over.body
    );
    let listed = list_tags(&router, &resource.arn).await;
    assert_eq!(
        listed.member(TAGS_ON_THE_WIRE),
        &filled,
        "case merge past the maximum: expected the stored map untouched, got {:?}",
        listed.member(TAGS_ON_THE_WIRE)
    );
    // A write that only REPLACES stored keys stays inside the bound.
    let replaced = tag_resource(&router, &resource.arn, &json!({ "key-000": "replaced" })).await;
    assert_eq!(
        replaced.status, 204,
        "case replacing a stored key at the maximum count: expected 204, got {} body {:?}",
        replaced.status, replaced.body
    );
}

/// RULE: a `ResourceArn` that is not an Amazon Resource Name at all is a 400
/// `ValidationException` on every operation of the tagging surface — such a value
/// cannot name a resource of any service, so the request is unanswerable rather than
/// the resource absent.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_at_all_is_a_400() {
    let router = emulator();
    let resource = fresh_resource(&router, Kind::DataSet, "arn-refusals", None).await;

    // Rows differ along WHAT the caller sent instead of an ARN: a bare name, a bare
    // identifier, a path, a URL, and an ARN whose scheme prefix is misspelled.
    let rows = vec![
        ("a bare name", "a-plain-string".to_string()),
        ("a bare identifier", ABSENT_ID.to_string()),
        ("a resource path with no ARN prefix", format!("data-sets/{ABSENT_ID}")),
        (
            "a URL",
            format!("https://dataexchange.{REGION}.amazonaws.com/v1/data-sets/{ABSENT_ID}"),
        ),
        (
            "an ARN whose scheme is misspelled",
            format!("urn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{ABSENT_ID}"),
        ),
        ("the shortest non-ARN there is", "a".to_string()),
    ];

    for (how, value) in &rows {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let case = format!("{operation}: {how}");
            let answer = match operation {
                "ListTagsForResource" => list_tags(&router, value).await,
                "TagResource" => tag_resource(&router, value, &json!({ "team": "data" })).await,
                _ => untag_resource(&router, value, &["team"]).await,
            };
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 for {value:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException for {value:?}, got {:?}",
                answer.error
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let listed = list_tags(&router, &resource.arn).await;
    assert_eq!(
        listed.member(TAGS_ON_THE_WIRE),
        &json!({}),
        "case after the refusals: expected the stored resource untouched, got {:?}",
        listed.member(TAGS_ON_THE_WIRE)
    );
}

/// RULE: an ARN that addresses no resource this emulator holds is a 404
/// `ResourceNotFoundException` reporting the ARN as the absent `ResourceId`, on every
/// operation of the tagging surface — including an ARN whose ids are real but whose
/// partition, region or account is not this resource's, because the ARN a resource was
/// minted with is its identity.
#[tokio::test]
async fn an_arn_addressing_nothing_stored_is_a_404() {
    let router = emulator();
    let data_set = create_data_set(&router, "absent-tags", "a", "S3_SNAPSHOT", None).await;
    let data_set_id = data_set.string("Id");
    let data_set_arn = data_set.string("Arn");
    let revision = create_revision(&router, &data_set_id, None, None, "setup").await;
    let revision_arn = revision.string("Arn");

    // Rows differ along WHICH part of the ARN addresses nothing: the data set id, the
    // revision id, the region, the account, the partition, the vendor, and the kind of
    // resource named — a job (whose create carries no `Tags`), an event action id this
    // emulator never minted and a data grant (taggable per the model, but minted by an
    // operation that is still a scaffold, so this emulator stores none), and resource
    // paths that name no id at all or go deeper than any resource this service holds.
    let rows = vec![
        (
            "an unknown data set",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{ABSENT_ID}"),
        ),
        (
            "a known data set in another region",
            data_set_arn.replace(REGION, "eu-west-1"),
        ),
        (
            "a known data set under another account",
            data_set_arn.replace(ACCOUNT, "123456789012"),
        ),
        (
            "a known data set in another partition",
            data_set_arn.replace("arn:aws:", "arn:aws-cn:"),
        ),
        (
            "an unknown revision of a known data set",
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}/revisions/\
                 {ABSENT_ID}"
            ),
        ),
        (
            "a known revision under an unknown data set",
            revision_arn.replace(&data_set_id, ABSENT_ID),
        ),
        (
            "a known revision in another region",
            revision_arn.replace(REGION, "eu-west-1"),
        ),
        (
            "another vendor's ARN",
            format!("arn:aws:s3:::my-bucket/data-sets/{ABSENT_ID}"),
        ),
        (
            "a job, a kind whose create carries no Tags",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:jobs/{ABSENT_ID}"),
        ),
        (
            "an event action id this emulator never minted",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:event-actions/{ABSENT_ID}"),
        ),
        (
            "a data grant, a taggable kind nothing is stored of",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-grants/{ABSENT_ID}"),
        ),
        (
            "an ARN with no resource field",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:"),
        ),
        (
            "a data set path with no id",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/"),
        ),
        (
            "a resource path deeper than any resource this service holds",
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}/revisions/\
                 {ABSENT_ID}/assets/{ABSENT_ID}"
            ),
        ),
    ];

    for (how, arn) in &rows {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let case = format!("{operation}: {how}");
            let answer = match operation {
                "ListTagsForResource" => list_tags(&router, arn).await,
                "TagResource" => tag_resource(&router, arn, &json!({ "team": "data" })).await,
                _ => untag_resource(&router, arn, &["team"]).await,
            };
            assert_eq!(
                answer.status, 404,
                "case {case}: expected 404 for {arn:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException for {arn:?}, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.member("ResourceId"),
                &json!(arn),
                "case {case}: expected the 404 to report the ARN it was asked about, got {:?}",
                answer.member("ResourceId")
            );
        }
    }

    // The resources that DO exist were left alone by every one of those answers.
    for (case, arn) in [
        ("the data set", &data_set_arn),
        ("its revision", &revision_arn),
    ] {
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case} after the 404s: expected 200, got {} body {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.member(TAGS_ON_THE_WIRE),
            &json!({}),
            "case {case} after the 404s: expected an untouched empty tag map, got {:?}",
            listed.member(TAGS_ON_THE_WIRE)
        );
    }

    // A resource that is deleted stops being addressable, and so does its revision.
    let deleted = call(
        &router,
        "DELETE",
        &format!("/v1/data-sets/{data_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case delete setup: expected 204 from DeleteDataSet, got {} body {:?}",
        deleted.status, deleted.body
    );
    for (case, arn) in [
        ("the deleted data set", &data_set_arn),
        ("a revision of the deleted data set", &revision_arn),
    ] {
        let answer = list_tags(&router, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404 after the delete, got {} body {:?}",
            answer.status, answer.body
        );
    }
}
