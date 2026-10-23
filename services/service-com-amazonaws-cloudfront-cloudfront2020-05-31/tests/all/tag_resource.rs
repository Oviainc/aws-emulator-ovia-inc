// SPDX-License-Identifier: CC0-1.0

//! `TagResource` — adding tags to a CloudFront resource, end to end over HTTP.
//!
//! The rule under test: a tagging call ADDS the tags it names to the set the resource its
//! ARN identifies already carries — a key the resource already carries takes the value
//! the call names, every other key it carries is left exactly as it was, and the
//! resulting set is what a tag read reports — for every resource kind the store holds and
//! tags; an ARN the store holds no resource for is `NoSuchResource` (404), a tag set no
//! resource can carry is refused (`InvalidTagging` 400 for the rules the model cannot
//! state, `ValidationException` 400 for the ones it can) and leaves the resource carrying
//! exactly the tags it carried before, and a tag set is associated with the resource
//! rather than part of its configuration, so tagging mints no new version token.
//!
//! Every row below differs along what that rule generalises over: how many tags the call
//! names (none, one, several, enough to fill the published maximum), how many the
//! resource already carries, whether the added keys overlap the carried ones, whether a
//! tag names a `Value` and whether that value is empty, which characters the key and the
//! value use, their length, the order the tags are submitted in, and which kind of
//! resource the ARN names.
//!
//! The harness (the router, the request helpers, the element scanner, the configuration
//! table and the tag helpers) is the lifecycle and `distribution_tags` suites', reused
//! rather than copied.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_TAGS_PER_RESOURCE, PROPAGATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, delete_distribution, emulator, get_distribution, Answer, Config, ACCOUNT_ID, API_VERSION,
};
use crate::distribution_tags::{
    config_for, create_distribution_with_tags, list_tags_for_resource, reported_order,
    reported_tags, tag, tags_xml, ReportedTags, SubmittedTag,
};

// ---------------------------------------------------------------------------
// Calling the tagging surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/tagging?Operation=Tag&Resource=<arn>`, whose payload is a `Tags`.
///
/// The ARN travels in the query exactly as the reader's does: every character a
/// CloudFront ARN uses is legal in a query value, and none of them is `&`, `=` or `%`.
async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    submitted: Option<&[SubmittedTag]>,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/tagging?Operation=Tag&Resource={arn}"),
        Some(&tags_xml(submitted)),
        None,
    )
    .await
}

/// Create a key value store carrying `submitted`, and report its ARN.
///
/// `KeyValueStoreName`'s pattern is `^[a-zA-Z0-9-_]{1,64}$`, so the name is not a place
/// to vary characters — the tag set is what each row varies.
async fn create_key_value_store(
    router: &MultiServiceRouter,
    name: &str,
    submitted: Option<&[SubmittedTag]>,
) -> Answer {
    let body = format!(
        "<CreateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><Comment>tag-resource suite</Comment>{}\
         </CreateKeyValueStoreRequest>",
        tags_xml(submitted)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/key-value-store"),
        Some(&body),
        None,
    )
    .await
}

/// The tag set `arn` reports, read back through `ListTagsForResource`.
async fn tags_of(router: &MultiServiceRouter, arn: &str, case: &str) -> ReportedTags {
    let answer = list_tags_for_resource(router, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListTagsForResource on {arn} to answer 200, got {} ({})",
        answer.status, answer.raw
    );
    reported_tags(answer.root())
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One add: the tag set the resource is created with, and the tag set a `TagResource`
/// call then names.
#[derive(Debug, Clone)]
struct AddCase {
    case: &'static str,
    /// The resource's tag set at create time. `None` creates it with no `Items` at all.
    carried: Option<Vec<SubmittedTag>>,
    /// What the `TagResource` call names. `None` names no `Items` at all.
    added: Option<Vec<SubmittedTag>>,
}

impl AddCase {
    /// The tag set a read must report after the add: what the resource carried, with the
    /// added keys taking the values the call named.
    fn expected(&self) -> ReportedTags {
        let mut expected: ReportedTags =
            self.carried.clone().unwrap_or_default().into_iter().collect();
        for (key, value) in self.added.clone().unwrap_or_default() {
            expected.insert(key, value);
        }
        expected
    }

    /// The tag set the resource carries before the add.
    fn before(&self) -> ReportedTags {
        self.carried.clone().unwrap_or_default().into_iter().collect()
    }
}

fn tags(items: &[SubmittedTag]) -> Option<Vec<SubmittedTag>> {
    Some(items.to_vec())
}

/// `count` distinct tags whose keys are prefixed with `prefix`.
fn generated(prefix: &str, count: usize) -> Vec<SubmittedTag> {
    (0..count)
        .map(|index| tag(&format!("{prefix}-{index:03}"), Some(&format!("value-{index}"))))
        .collect()
}

/// The table every add assertion runs over.
///
/// The character and length rows are built from what `TagKey` and `TagValue` document:
/// "valid characters include `a-z`, `A-Z`, `0-9`, space, and the special characters
/// `_ - . : / = + @`", a key length "between 1 and 128 characters" and a value length
/// "between 0 and 256 characters"; each bound is a row here and one PAST each bound is a
/// row of [`tag_sets_no_resource_can_carry`]. `MAX_TAGS_PER_RESOURCE` is the count the
/// union of the carried and the added keys is bounded by, so a row fills it exactly and
/// another row of the refusal table steps one past it.
fn add_table() -> Vec<AddCase> {
    vec![
        AddCase {
            case: "one-tag-onto-an-untagged-resource",
            carried: None,
            added: tags(&[tag("environment", Some("production"))]),
        },
        AddCase {
            case: "no-items-at-all-changes-nothing",
            carried: tags(&[tag("environment", Some("production"))]),
            added: None,
        },
        AddCase {
            case: "empty-items-changes-nothing",
            carried: tags(&[tag("environment", Some("production"))]),
            added: Some(Vec::new()),
        },
        AddCase {
            case: "a-disjoint-key-is-added",
            carried: tags(&[tag("keep", Some("yes"))]),
            added: tags(&[tag("team", Some("platform"))]),
        },
        AddCase {
            case: "value-left-out",
            carried: tags(&[tag("keep", Some("yes"))]),
            added: tags(&[tag("team", None)]),
        },
        AddCase {
            case: "empty-value",
            carried: tags(&[tag("keep", Some("yes"))]),
            added: tags(&[tag("owner", Some(""))]),
        },
        AddCase {
            case: "a-carried-key-takes-the-new-value",
            carried: tags(&[tag("stage", Some("alpha")), tag("keep", Some("yes"))]),
            added: tags(&[tag("stage", Some("beta"))]),
        },
        AddCase {
            case: "a-carried-key-takes-no-value",
            carried: tags(&[tag("stage", Some("alpha")), tag("keep", Some("yes"))]),
            added: tags(&[tag("stage", None)]),
        },
        AddCase {
            case: "special-characters",
            carried: tags(&[tag("keep", Some("yes"))]),
            added: tags(&[tag(
                "cost.center:unit/01=a+b-c@d e",
                Some("a b_c-d.e:f/g=h+i@j"),
            )]),
        },
        AddCase {
            case: "unicode-letters",
            carried: None,
            added: tags(&[tag("proyecto", Some("producción"))]),
        },
        AddCase {
            // Submitted in descending key order, so a row reported in key order cannot
            // be passing by echoing the request's order.
            case: "three-tags-descending",
            carried: tags(&[tag("app", Some("legacy"))]),
            added: tags(&[
                tag("zone", Some("eu")),
                tag("mode", None),
                tag("app", Some("checkout")),
            ]),
        },
        AddCase {
            case: "key-at-minimum-length",
            carried: None,
            added: tags(&[tag("k", Some("v"))]),
        },
        AddCase {
            case: "key-and-value-at-maximum-length",
            carried: tags(&[tag("keep", Some("yes"))]),
            added: tags(&[tag(&"k".repeat(128), Some(&"v".repeat(256)))]),
        },
        AddCase {
            case: "many-onto-many-partly-overlapping",
            carried: tags(&[
                tag("a", Some("1")),
                tag("b", Some("2")),
                tag("c", None),
                tag("d", Some("4")),
            ]),
            added: tags(&[
                tag("c", Some("3")),
                tag("d", None),
                tag("e", Some("5")),
                tag("f", Some("")),
            ]),
        },
        AddCase {
            case: "fills-the-maximum-tag-count",
            carried: tags(&generated("carried", MAX_TAGS_PER_RESOURCE / 2)),
            added: tags(&generated("added", MAX_TAGS_PER_RESOURCE / 2)),
        },
        AddCase {
            case: "re-tags-every-key-of-a-full-tag-set",
            carried: tags(&generated("carried", MAX_TAGS_PER_RESOURCE)),
            added: Some(
                generated("carried", MAX_TAGS_PER_RESOURCE)
                    .into_iter()
                    .map(|(key, _)| (key, Some("rewritten".to_string())))
                    .collect(),
            ),
        },
    ]
}

/// A tag set no resource can carry, as a `TagResource` call names it against a resource
/// already carrying `carried`: `(case, carried, added, status, error code)`.
///
/// The `InvalidTagging` rows are the rules the model cannot state — at most one tag per
/// key, and at most [`MAX_TAGS_PER_RESOURCE`] tags counted over the union of the carried
/// and the added keys — and the `ValidationException` rows are one past `TagKey`'s
/// documented maximum length (128) and one past `TagValue`'s (256).
#[allow(clippy::type_complexity)]
fn tag_sets_no_resource_can_carry() -> Vec<(
    &'static str,
    Vec<SubmittedTag>,
    Vec<SubmittedTag>,
    u16,
    &'static str,
)> {
    vec![
        (
            "duplicate-key",
            vec![tag("keep", Some("yes"))],
            vec![tag("stage", Some("alpha")), tag("stage", Some("beta"))],
            400,
            "InvalidTagging",
        ),
        (
            "duplicate-key-among-many",
            vec![tag("keep", Some("yes"))],
            vec![
                tag("a", Some("1")),
                tag("b", Some("2")),
                tag("a", Some("3")),
                tag("c", None),
            ],
            400,
            "InvalidTagging",
        ),
        (
            "one-past-the-maximum-tag-count-in-one-call",
            vec![tag("keep", Some("yes"))],
            generated("added", MAX_TAGS_PER_RESOURCE + 1),
            400,
            "InvalidTagging",
        ),
        (
            // The bound is over the UNION: a full tag set takes no further key.
            "one-past-the-maximum-tag-count-over-the-union",
            generated("carried", MAX_TAGS_PER_RESOURCE),
            vec![tag("one-more", Some("v"))],
            400,
            "InvalidTagging",
        ),
        (
            "key-one-past-its-maximum-length",
            vec![tag("keep", Some("yes"))],
            vec![tag(&"k".repeat(129), Some("v"))],
            400,
            "ValidationException",
        ),
        (
            "value-one-past-its-maximum-length",
            vec![tag("keep", Some("yes"))],
            vec![tag("size", Some(&"v".repeat(257)))],
            400,
            "ValidationException",
        ),
    ]
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// `TagResource` adds the tags it names to the set the distribution already carries, and
/// a tag read reports the result: the added keys carry the values the call named, every
/// other carried key is untouched, and no other key appears.
#[tokio::test]
async fn tag_resource_adds_the_tags_it_names() {
    let router = emulator();

    for (index, row) in add_table().into_iter().enumerate() {
        let case = row.case;
        let config = config_for(index, &format!("ref-add-{index}"));
        let created =
            create_distribution_with_tags(&router, &config, row.carried.as_deref()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        let before = tags_of(&router, &arn, case).await;
        assert_eq!(
            before,
            row.before(),
            "case {case}: expected the create to leave {:?}, got {before:?}",
            row.before()
        );

        let tagged = tag_resource(&router, &arn, row.added.as_deref()).await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: expected TagResource to answer 204, got {} ({})",
            tagged.status, tagged.raw
        );
        assert!(
            tagged.raw.trim().is_empty(),
            "case {case}: expected TagResource to answer no body, got {:?}",
            tagged.raw
        );

        // The resulting set, read back through the reader.
        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after,
            row.expected(),
            "case {case}: expected the tag set {:?}, got {after:?}",
            row.expected()
        );
        // The same rule, stated member by member rather than as one map: every added key
        // carries the value the call named, and every carried key the call did not name
        // is exactly as it was.
        for (key, value) in row.added.clone().unwrap_or_default() {
            assert_eq!(
                after.get(&key),
                Some(&value),
                "case {case}: expected the added key {key:?} to carry {value:?}, got {:?}",
                after.get(&key)
            );
        }
        for (key, value) in &before {
            if row
                .added
                .clone()
                .unwrap_or_default()
                .iter()
                .any(|(added, _)| added == key)
            {
                continue;
            }
            assert_eq!(
                after.get(key),
                Some(value),
                "case {case}: expected the carried key {key:?} to still carry {value:?}, got {:?}",
                after.get(key)
            );
        }
        // A tag set is keyed by tag key, so it is reported in key order whatever order
        // the request listed it in.
        let read = list_tags_for_resource(&router, &arn).await;
        let order = reported_order(read.root());
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(
            order, sorted,
            "case {case}: expected the tags in key order, got {order:?}"
        );

        // Adding the same tags again adds nothing: the call is idempotent in the set it
        // leaves behind.
        let again = tag_resource(&router, &arn, row.added.as_deref()).await;
        assert_eq!(
            again.status, 204,
            "case {case}: expected the repeated TagResource to answer 204, got {} ({})",
            again.status, again.raw
        );
        let repeated = tags_of(&router, &arn, case).await;
        assert_eq!(
            repeated, after,
            "case {case}: expected the repeat to leave {after:?}, got {repeated:?}"
        );
    }
}

/// A tagging call is about the ARN, not about one kind of resource: a key value store
/// this emulator holds takes the same add, reported by the same reader.
#[tokio::test]
async fn tag_resource_resolves_every_resource_kind_the_store_holds() {
    let router = emulator();

    for (index, row) in add_table().into_iter().enumerate() {
        let case = row.case;
        let name = format!("store-{index:02}");
        let created = create_key_value_store(&router, &name, row.carried.as_deref()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the key value store create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/")),
            "case {case}: expected a key value store ARN, got {arn:?}"
        );

        let tagged = tag_resource(&router, &arn, row.added.as_deref()).await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: expected TagResource to answer 204 for {arn}, got {} ({})",
            tagged.status, tagged.raw
        );
        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after,
            row.expected(),
            "case {case}: expected {arn} to report {:?}, got {after:?}",
            row.expected()
        );
    }
}

/// A tag set no resource can carry is refused, and the resource keeps exactly the tags it
/// carried before the refused call.
#[tokio::test]
async fn a_tag_set_no_resource_can_carry_is_refused_and_changes_nothing() {
    let router = emulator();

    for (index, (case, carried, added, status, code)) in
        tag_sets_no_resource_can_carry().into_iter().enumerate()
    {
        let config = config_for(index, &format!("ref-refused-{index}"));
        let created = create_distribution_with_tags(&router, &config, Some(&carried)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
        let before = tags_of(&router, &arn, case).await;

        let refused = tag_resource(&router, &arn, Some(&added)).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected {status}, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {:?}",
            refused.error_code()
        );

        let after = tags_of(&router, &arn, case).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused call to leave {before:?}, got {after:?}"
        );
    }

    // A full tag set still takes a NEW VALUE for a key it already carries: the bound is
    // over the union of the keys, so this is not one tag more.
    let config = config_for(0, "ref-full-rewrite");
    let full = generated("carried", MAX_TAGS_PER_RESOURCE);
    let created = create_distribution_with_tags(&router, &config, Some(&full)).await;
    assert_eq!(
        created.status, 201,
        "case full-set-rewrite: expected the create to answer 201, got {} ({})",
        created.status, created.raw
    );
    let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
    let rewritten = tag_resource(
        &router,
        &arn,
        Some(&[tag("carried-000", Some("rewritten"))]),
    )
    .await;
    assert_eq!(
        rewritten.status, 204,
        "case full-set-rewrite: expected 204, got {} ({})",
        rewritten.status, rewritten.raw
    );
    let after = tags_of(&router, &arn, "full-set-rewrite").await;
    assert_eq!(
        after.len(),
        MAX_TAGS_PER_RESOURCE,
        "case full-set-rewrite: expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        after.len()
    );
    assert_eq!(
        after.get("carried-000"),
        Some(&Some("rewritten".to_string())),
        "case full-set-rewrite: expected the rewritten value, got {:?}",
        after.get("carried-000")
    );
}

/// An ARN this emulator holds no resource for is `NoSuchResource`, whichever kind of
/// CloudFront resource it names — before the resource exists, for a resource kind the
/// store holds none of, and after the resource is deleted — and such a call persists
/// nothing.
#[tokio::test]
async fn tag_resource_refuses_an_arn_no_resource_carries() {
    let router = emulator();

    // Every row is a well-formed CloudFront ARN (`ResourceARN`'s pattern is satisfied),
    // so what makes each a refusal is that the store holds no resource carrying it.
    let unknown = [
        (
            "distribution-that-was-never-created",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E00000000000A"),
        ),
        (
            "key-value-store-that-was-never-created",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/absent-store"),
        ),
        (
            // A resource kind this emulator holds none of at all.
            "cache-policy",
            format!(
                "arn:aws:cloudfront::{ACCOUNT_ID}:cache-policy/11111111-2222-3333-4444-555555555555"
            ),
        ),
        (
            "function",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:function/absent-function"),
        ),
        (
            // Another partition: the same resource part, a different ARN.
            "another-partition",
            format!("arn:aws-cn:cloudfront::{ACCOUNT_ID}:distribution/E00000000000A"),
        ),
    ];

    for (case, arn) in &unknown {
        let refused = tag_resource(&router, arn, Some(&[tag("environment", Some("test"))])).await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected 404, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchResource",
            "case {case}: expected NoSuchResource, got {:?}",
            refused.error_code()
        );
        // Nothing was persisted under that ARN: the read refuses it too.
        let read = list_tags_for_resource(&router, arn).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected the ARN to still name no resource, got {} ({})",
            read.status, read.raw
        );
    }

    // A tag set lives on the resource record, so a deleted resource's ARN names nothing
    // again — including for a tagging call.
    let disabled = Config {
        enabled: false,
        ..config_for(0, "ref-deleted")
    };
    let created =
        create_distribution_with_tags(&router, &disabled, Some(&[tag("keep", Some("yes"))])).await;
    assert_eq!(
        created.status, 201,
        "case deleted: expected the create to answer 201, got {} ({})",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    // The distribution was created disabled, and a delete lands only once that
    // configuration has also finished propagating, so the clock is given its span first.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;
    let deleted = delete_distribution(&router, &id, Some(&etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case deleted: expected the delete to answer 204, got {} ({})",
        deleted.status, deleted.raw
    );
    let refused = tag_resource(&router, &arn, Some(&[tag("environment", Some("test"))])).await;
    assert_eq!(
        refused.status, 404,
        "case deleted: expected 404, got {} ({})",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "NoSuchResource",
        "case deleted: expected NoSuchResource, got {:?}",
        refused.error_code()
    );
}

/// A tag set is associated with the resource rather than part of its configuration:
/// tagging one resource mints no new version token, changes nothing a configuration read
/// reports, and leaves every OTHER resource's tag set alone.
#[tokio::test]
async fn tagging_touches_only_the_tagged_resource() {
    let router = emulator();

    // Three distributions, each created with its own tag set, plus a key value store, so
    // a tagging call has neighbours of both kinds to leave alone.
    let mut distributions: Vec<(String, String, String, ReportedTags)> = Vec::new();
    for index in 0..3 {
        let config = config_for(index, &format!("ref-neighbour-{index}"));
        let carried = vec![tag(&format!("owner-{index}"), Some("original"))];
        let created = create_distribution_with_tags(&router, &config, Some(&carried)).await;
        assert_eq!(
            created.status, 201,
            "case neighbour-{index}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        distributions.push((
            created.root().text_at("Id").unwrap_or_default().to_string(),
            created.root().text_at("ARN").unwrap_or_default().to_string(),
            created.header("etag").to_string(),
            carried.into_iter().collect::<BTreeMap<_, _>>(),
        ));
    }
    let store = create_key_value_store(&router, "neighbour-store", Some(&[tag("kind", Some("kvs"))]))
        .await;
    assert_eq!(
        store.status, 201,
        "case neighbour-store: expected the create to answer 201, got {} ({})",
        store.status, store.raw
    );
    let store_arn = store.root().text_at("ARN").unwrap_or_default().to_string();

    // Tag the middle one only.
    let (id, arn, etag, carried) = distributions[1].clone();
    let tagged = tag_resource(&router, &arn, Some(&[tag("added", Some("later"))])).await;
    assert_eq!(
        tagged.status, 204,
        "case tag-the-middle: expected 204, got {} ({})",
        tagged.status, tagged.raw
    );

    // Its own configuration is untouched, and so is its version token: a tag set is not
    // part of the configuration a version names.
    let read = get_distribution(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case tag-the-middle: expected GetDistribution to answer 200, got {} ({})",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case tag-the-middle: expected the version token {etag} to be unchanged, got {:?}",
        read.header("etag")
    );
    let config = config_for(1, "ref-neighbour-1");
    let read_config = read
        .root()
        .at("DistributionConfig")
        .expect("case tag-the-middle: GetDistribution answered no config");
    config.assert_matches(read_config, "get after tagging");

    let mut expected = carried.clone();
    expected.insert("added".to_string(), Some("later".to_string()));
    let after = tags_of(&router, &arn, "tag-the-middle").await;
    assert_eq!(
        after, expected,
        "case tag-the-middle: expected {expected:?}, got {after:?}"
    );

    // Every neighbour still reports exactly its own tag set.
    for (index, (_, other_arn, _, other_tags)) in distributions.iter().enumerate() {
        if index == 1 {
            continue;
        }
        let neighbour = tags_of(&router, other_arn, "neighbour").await;
        assert_eq!(
            &neighbour, other_tags,
            "case neighbour-{index}: expected {other_arn} to still report {other_tags:?}, got {neighbour:?}"
        );
    }
    let store_tags = tags_of(&router, &store_arn, "neighbour-store").await;
    assert_eq!(
        store_tags,
        BTreeMap::from([("kind".to_string(), Some("kvs".to_string()))]),
        "case neighbour-store: expected the store to report only its own tag, got {store_tags:?}"
    );
}
