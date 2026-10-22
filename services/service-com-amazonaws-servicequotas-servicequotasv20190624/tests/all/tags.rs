// SPDX-License-Identifier: CC0-1.0

//! Tagging an applied quota, end to end over the wire: `TagResource`,
//! `ListTagsForResource` and `UntagResource`.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine — so what these tests exercise is the
//! HTTP behaviour and the persisted state behind it, not a handler called directly.
//!
//! The rules, each asserted over a TABLE of inputs rather than the one recorded case:
//!
//! * a resource's tags are a MAP keyed by `TagKey`: `TagResource` upserts (a key
//!   already present keeps one tag and takes the new value), `UntagResource` removes
//!   exactly the keys it names and is idempotent, and `ListTagsForResource` reports
//!   exactly the surviving set — whatever the tags' size, characters, count or the
//!   order they were given in;
//! * a tag set belongs to the ARN, not to the endpoint that wrote it: the same
//!   `ResourceARN` reads back the same tags from any regional endpoint, a global
//!   quota (whose ARN carries no region) is ONE resource account-wide, and two
//!   distinct quota ARNs keep independent sets;
//! * the ceiling is 50 tags per resource (the Service Quotas User Guide's tag
//!   restrictions, the page `TooManyTagsException` points at), and a refused call
//!   leaves the set it found;
//! * a `ResourceARN` that is not an applied-quota ARN is `IllegalArgumentException`
//!   and one that names no quota of this account is `NoSuchResourceException`, the
//!   same way for all three operations;
//! * a `TagKey` in the `aws:` namespace AWS reserves for itself is
//!   `TagPolicyViolationException`.
//!
//! Service Quotas speaks `awsJson1_1`, so every modelled client error is HTTP 400 and
//! the body's `__type` tells one from another (see
//! `service_com_amazonaws_servicequotas_servicequotasv20190624::scaffolded::wire_http_status`).
//!
//! No quota pair is hardcoded: the quotas under test are discovered through
//! `ListServices` + `ListServiceQuotas`, and their ARNs are the `QuotaArn` the API
//! itself reports — so this suite keeps holding as the seeded catalog grows.
//!
//! The harness (`client`, `emulator`, `walk_pages`, `ACCOUNT`) is the one
//! `quota_workflow.rs` already uses, so every suite speaks to the emulator the same
//! way; only the three tagging calls are added here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};

/// The status an awsJson front end answers every modelled CLIENT error on.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The most tags one resource may carry
/// (`crate::state::MAX_TAGS_PER_RESOURCE`, from the Service Quotas User Guide's tag
/// restrictions).
const MAX_TAGS: usize = 50;

/// One tag, the way the model's `Tag` shape carries it: `(Key, Value)`.
type TagPair = (String, String);

/// A tag set, as `ListTagsForResource` describes it: `TagKey` -> `TagValue`.
type TagSet = BTreeMap<String, String>;

/// One row of the tag-shape table: the case, the tags as sent, and the set the
/// resource must end up carrying.
type TagRow = (&'static str, Vec<TagPair>, TagSet);

/// One row of the upsert table: the case, the first call's tags, the second call's,
/// and the set the two together must leave.
type UpsertRow = (&'static str, Vec<TagPair>, Vec<TagPair>, TagSet);

/// The tagging calls this suite adds to the shared client.
impl Client {
    /// `ListTagsForResource` for `arn`, as the map it describes.
    ///
    /// Asserts the `Tags` member is PRESENT — the model's `OutputTags` is len 0..200,
    /// so an untagged resource is an empty list rather than an absent member.
    async fn tags(&self, arn: &str) -> TagSet {
        let body = self
            .ok("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        let listed = body
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case ListTagsForResource {arn}: expected a Tags list, got {body}"))
            .clone();
        listed
            .iter()
            .map(|tag| {
                (
                    tag["Key"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {arn}: a tag with no Key: {tag}"))
                        .to_string(),
                    tag["Value"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {arn}: a tag with no Value: {tag}"))
                        .to_string(),
                )
            })
            .collect()
    }

    /// `TagResource` for `arn` with `tags`, which must succeed.
    async fn tag(&self, arn: &str, tags: &[TagPair]) {
        self.ok("TagResource", tag_request(arn, tags)).await;
    }

    /// `UntagResource` for `arn` with `keys`, which must succeed.
    async fn untag(&self, arn: &str, keys: &[String]) {
        self.ok(
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": keys}),
        )
        .await;
    }
}

/// A `TagResource` request body.
fn tag_request(arn: &str, tags: &[TagPair]) -> Value {
    json!({
        "ResourceARN": arn,
        "Tags": tags
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect::<Vec<_>>(),
    })
}

/// One applied quota, as the API reports it.
#[derive(Debug, Clone)]
struct CatalogQuota {
    service_code: String,
    quota_code: String,
    /// `GlobalQuota`: counted account-wide, so its ARN carries no region.
    global: bool,
    /// `QuotaArn`, straight out of the listing — never hand-built here.
    arn: String,
}

/// Every applied quota this emulator's catalog holds, discovered through the API and
/// walked to the end of every page with the shared `walk_pages` helper.
async fn catalog(client: &Client) -> Vec<CatalogQuota> {
    let service_codes: Vec<String> = walk_pages(client, "ListServices", &json!({}), "Services", None)
        .await
        .iter()
        .map(|service| {
            service["ServiceCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case ListServices: a service with no ServiceCode: {service}"))
                .to_string()
        })
        .collect();

    let mut quotas = Vec::new();
    for service_code in service_codes {
        let listed = walk_pages(
            client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        for quota in listed {
            quotas.push(CatalogQuota {
                service_code: quota["ServiceCode"].as_str().unwrap_or_default().to_string(),
                quota_code: quota["QuotaCode"].as_str().unwrap_or_default().to_string(),
                global: quota["GlobalQuota"].as_bool().unwrap_or(false),
                arn: quota["QuotaArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case ListServiceQuotas: a quota with no QuotaArn: {quota}")
                    })
                    .to_string(),
            });
        }
    }
    assert!(
        quotas.len() >= 2,
        "case catalog: the tagging rules need at least two applied quotas, got {}",
        quotas.len()
    );
    quotas
}

/// The first quota in the catalog that is NOT a `GlobalQuota` — the regional case,
/// whose ARN carries a region.
fn regional(catalog: &[CatalogQuota]) -> CatalogQuota {
    catalog
        .iter()
        .find(|quota| !quota.global)
        .cloned()
        .expect("case catalog: the seeded catalog has at least one regional quota")
}

/// The first `GlobalQuota` in the catalog — the account-wide case, whose ARN carries
/// no region.
fn global(catalog: &[CatalogQuota]) -> CatalogQuota {
    catalog
        .iter()
        .find(|quota| quota.global)
        .cloned()
        .expect("case catalog: the seeded catalog has at least one GlobalQuota")
}

/// A tag list as the map it describes.
fn expected(tags: &[TagPair]) -> TagSet {
    tags.iter().cloned().collect()
}

/// `n` tags named `tag-00`.. — the generated rows the count rules need.
fn many_tags(n: usize) -> Vec<TagPair> {
    (0..n)
        .map(|index| (format!("tag-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// A tag pair, spelled once.
fn tag(key: &str, value: &str) -> TagPair {
    (key.to_string(), value.to_string())
}

/// The shapes of tag a `Tag` may carry, from the model's own constraints
/// (`smithy_query(query="shape",shape="Tag")`: `Key` len 1..128 and `Value` len
/// 0..256, both matching `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), plus the two counting
/// cases — several tags in one call, and a repeated key.
///
/// Each row is `(case, tags as sent, the set the resource must end up with)`.
fn tag_rows() -> Vec<TagRow> {
    let max_key = "K".repeat(128);
    let max_value = "V".repeat(256);
    let rows: Vec<(&'static str, Vec<TagPair>)> = vec![
        ("one plain tag", vec![tag("Environment", "Production")]),
        // `TagValue` is len 0..256: the empty value is inside the model's bound.
        ("an empty tag value", vec![tag("Owner", "")]),
        // Each published bound of `Tag`: `Key` at 128 characters, `Value` at 256.
        (
            "a key and a value at their maximum lengths",
            vec![(max_key.clone(), max_value.clone())],
        ),
        (
            "every special character the Tag pattern allows",
            vec![tag("key_.:/=+-@", "value_.:/=+-@")],
        ),
        // `\p{L}` (letters), `\p{Z}` (separators) and `\p{N}` (numbers) are not ASCII
        // only, and a key is bounded in CHARACTERS, not bytes.
        (
            "unicode letters, a space and a digit",
            vec![tag("Клиент 7", "produção 3")],
        ),
        (
            "three tags given in descending key order",
            vec![tag("c", "3"), tag("b", "2"), tag("a", "1")],
        ),
        ("ten tags in one call", many_tags(10)),
    ];
    let mut rows: Vec<_> = rows
        .into_iter()
        .map(|(case, tags)| {
            let want = expected(&tags);
            (case, tags, want)
        })
        .collect();
    // A tag set is keyed by `TagKey`, so a key repeated inside ONE request leaves one
    // tag carrying the last value given for it.
    rows.push((
        "a key repeated in one call",
        vec![tag("dup", "first"), tag("dup", "last")],
        expected(&[tag("dup", "last")]),
    ));
    rows
}

#[tokio::test]
async fn a_tag_set_round_trips_through_list_for_every_shape_of_tag() {
    for (case, sent, want) in tag_rows() {
        let engine = emulator();
        let client = client(&engine, "us-east-1");
        let quota = regional(&catalog(&client).await);

        // Nothing has tagged this quota: the set is empty, not absent.
        let before = client.tags(&quota.arn).await;
        assert!(
            before.is_empty(),
            "case {case:?}: an untagged quota must list no tags, got {before:?}"
        );

        client.tag(&quota.arn, &sent).await;

        let listed = client.tags(&quota.arn).await;
        assert_eq!(
            listed, want,
            "case {case:?}: expected the tag set {want:?} on {}, got {listed:?}",
            quota.arn
        );

        // Reading must not change what a later caller sees.
        let again = client.tags(&quota.arn).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a second read must report the same set {listed:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn tagging_a_key_again_replaces_its_value_and_leaves_the_others() {
    // (case, first call, second call, the set both calls together must leave)
    let rows: Vec<UpsertRow> = vec![
        (
            "the same key with a new value",
            vec![tag("Stage", "beta")],
            vec![tag("Stage", "ga")],
            expected(&[tag("Stage", "ga")]),
        ),
        (
            "a new key beside an existing one",
            vec![tag("Stage", "beta")],
            vec![tag("Owner", "team")],
            expected(&[tag("Owner", "team"), tag("Stage", "beta")]),
        ),
        (
            "one key replaced and one added",
            vec![tag("a", "1"), tag("b", "2")],
            vec![tag("b", "22"), tag("c", "3")],
            expected(&[tag("a", "1"), tag("b", "22"), tag("c", "3")]),
        ),
        (
            "the same value written twice",
            vec![tag("a", "1")],
            vec![tag("a", "1")],
            expected(&[tag("a", "1")]),
        ),
        (
            "a value replaced by the empty one",
            vec![tag("a", "1")],
            vec![tag("a", "")],
            expected(&[tag("a", "")]),
        ),
    ];

    for (case, first, second, want) in rows {
        let engine = emulator();
        let client = client(&engine, "eu-west-1");
        let quota = regional(&catalog(&client).await);

        client.tag(&quota.arn, &first).await;
        client.tag(&quota.arn, &second).await;

        let listed = client.tags(&quota.arn).await;
        assert_eq!(
            listed, want,
            "case {case:?}: expected {want:?} after tagging {first:?} then {second:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_and_repeats_harmlessly() {
    let initial = vec![tag("a", "1"), tag("b", "2"), tag("c", "3")];
    // (case, the keys removed, the keys that must survive)
    let rows: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        ("no keys at all", vec![], vec!["a", "b", "c"]),
        ("one key", vec!["a".to_string()], vec!["b", "c"]),
        (
            "two keys",
            vec!["a".to_string(), "c".to_string()],
            vec!["b"],
        ),
        (
            "a key the resource does not carry",
            vec!["zz".to_string()],
            vec!["a", "b", "c"],
        ),
        (
            "one present key and one absent key",
            vec!["b".to_string(), "zz".to_string()],
            vec!["a", "c"],
        ),
        (
            "every key",
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec![],
        ),
        (
            "the same key twice in one call",
            vec!["a".to_string(), "a".to_string()],
            vec!["b", "c"],
        ),
    ];

    for (case, removed, survivors) in rows {
        let engine = emulator();
        let client = client(&engine, "us-west-2");
        let quota = regional(&catalog(&client).await);
        client.tag(&quota.arn, &initial).await;

        let want: TagSet = initial
            .iter()
            .filter(|(key, _)| survivors.contains(&key.as_str()))
            .cloned()
            .collect();

        client.untag(&quota.arn, &removed).await;
        let listed = client.tags(&quota.arn).await;
        assert_eq!(
            listed, want,
            "case {case:?}: removing {removed:?} must leave {want:?}, got {listed:?}"
        );

        // Removal is idempotent: the same call again leaves the same set.
        client.untag(&quota.arn, &removed).await;
        let repeated = client.tags(&quota.arn).await;
        assert_eq!(
            repeated, want,
            "case {case:?}: removing {removed:?} twice must still leave {want:?}, got {repeated:?}"
        );

        // And what survived can still be read back and removed.
        client
            .tag(&quota.arn, &[tag("after", "untag")])
            .await;
        let reopened = client.tags(&quota.arn).await;
        assert_eq!(
            reopened.get("after").map(String::as_str),
            Some("untag"),
            "case {case:?}: a quota must still take tags after an untag, got {reopened:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_belongs_to_the_arn_not_to_the_endpoint_that_wrote_it() {
    let engine = emulator();
    let writer = client(&engine, "us-east-1");
    let reader = client(&engine, "ap-southeast-2");

    let here = catalog(&writer).await;
    let there = catalog(&reader).await;
    let regional_here = regional(&here);
    let regional_there = regional(&there);
    let global_here = global(&here);
    let global_there = global(&there);

    // A regional quota is a different resource at each regional endpoint; a global
    // quota is one resource account-wide, and reports the same (regionless) ARN
    // wherever it is read.
    assert_ne!(
        regional_here.arn, regional_there.arn,
        "case regional ARN: a regional quota's ARN must name its region, got {} at both endpoints",
        regional_here.arn
    );
    assert_eq!(
        global_here.arn, global_there.arn,
        "case global ARN: a GlobalQuota must report one ARN; got {} and {}",
        global_here.arn, global_there.arn
    );

    // (case, the ARN written through us-east-1, the ARN read through ap-southeast-2,
    //  whether the read must see the tag)
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "the same regional ARN read from another endpoint",
            &regional_here.arn,
            &regional_here.arn,
            true,
        ),
        (
            "the other region's ARN for the same quota",
            &regional_here.arn,
            &regional_there.arn,
            false,
        ),
        (
            "a global quota, written and read at different endpoints",
            &global_here.arn,
            &global_here.arn,
            true,
        ),
    ];

    for (index, (case, written, read, shared)) in rows.into_iter().enumerate() {
        // The key is unique by construction (the row's index), so one row's marker is
        // never mistaken for another's.
        let marker = tag(&format!("marker-{index}"), "set");
        writer.tag(written, std::slice::from_ref(&marker)).await;
        let listed = reader.tags(read).await;
        assert_eq!(
            listed.contains_key(&marker.0),
            shared,
            "case {case:?}: tagging {written} and reading {read} must{} share the tag, got {listed:?}",
            if shared { "" } else { " not" }
        );
        // Leave no tag behind for the next row.
        writer.untag(written, std::slice::from_ref(&marker.0)).await;
    }

    // Two distinct quotas keep independent sets.
    let other = here
        .iter()
        .find(|quota| quota.arn != regional_here.arn)
        .expect("case independence: the catalog holds a second quota")
        .clone();
    writer
        .tag(&regional_here.arn, &[tag("only", "first")])
        .await;
    let other_tags = writer.tags(&other.arn).await;
    assert!(
        other_tags.is_empty(),
        "case independence: tagging {} must not tag {}, got {other_tags:?}",
        regional_here.arn,
        other.arn
    );
}

#[tokio::test]
async fn fifty_tags_are_accepted_and_the_fifty_first_is_refused_without_writing() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let catalog = catalog(&client).await;
    let quota = regional(&catalog);
    let untouched = global(&catalog);

    // Each published bound: the ceiling itself is accepted...
    let at_ceiling = many_tags(MAX_TAGS);
    client.tag(&quota.arn, &at_ceiling).await;
    let listed = client.tags(&quota.arn).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case at the ceiling: {MAX_TAGS} tags must all be stored, got {}",
        listed.len()
    );

    // ...replacing a key at the ceiling is not growth, so it is still accepted...
    client
        .tag(&quota.arn, &[tag("tag-00", "replaced")])
        .await;
    let replaced = client.tags(&quota.arn).await;
    assert_eq!(
        (replaced.len(), replaced.get("tag-00").map(String::as_str)),
        (MAX_TAGS, Some("replaced")),
        "case replace at the ceiling: expected {MAX_TAGS} tags with tag-00 replaced, got {replaced:?}"
    );

    // ...and one key past it is refused, with the set it found left alone.
    let (status, code) = client
        .refused("TagResource", tag_request(&quota.arn, &[tag("one-too-many", "x")]))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR_STATUS, "TooManyTagsException"),
        "case one past the ceiling: expected {CLIENT_ERROR_STATUS} TooManyTagsException, got {status} {code}"
    );
    let after = client.tags(&quota.arn).await;
    assert_eq!(
        after, replaced,
        "case one past the ceiling: a refused call must write nothing; expected {replaced:?}, got {after:?}"
    );

    // One call that is itself past the ceiling writes nothing at all.
    let (status, code) = client
        .refused(
            "TagResource",
            tag_request(&untouched.arn, &many_tags(MAX_TAGS + 1)),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR_STATUS, "TooManyTagsException"),
        "case {} tags in one call: expected {CLIENT_ERROR_STATUS} TooManyTagsException, got {status} {code}",
        MAX_TAGS + 1
    );
    let nothing = client.tags(&untouched.arn).await;
    assert!(
        nothing.is_empty(),
        "case {} tags in one call: nothing may be written, got {nothing:?}",
        MAX_TAGS + 1
    );
}

#[tokio::test]
async fn a_resource_arn_that_names_no_applied_quota_is_refused_the_same_way_everywhere() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let catalog = catalog(&client).await;
    let quota = regional(&catalog);
    let account_wide = global(&catalog);

    // (case, ResourceARN, the modelled error it must be refused with)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a resource part that is not <ServiceCode>/<QuotaCode>",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:notaquota"),
            "IllegalArgumentException",
        ),
        (
            "another service's arnNamespace",
            format!("arn:aws:dynamodb:us-east-1:{ACCOUNT}:table/things"),
            "IllegalArgumentException",
        ),
        (
            "a partition that is not an AWS one",
            format!(
                "arn:notaws:servicequotas:us-east-1:{ACCOUNT}:{}/{}",
                quota.service_code, quota.quota_code
            ),
            "IllegalArgumentException",
        ),
        (
            "an empty quota code",
            format!("arn:aws:servicequotas:us-east-1:{ACCOUNT}:{}/", quota.service_code),
            "IllegalArgumentException",
        ),
        (
            "a quota code the catalog does not hold",
            format!(
                "arn:aws:servicequotas:us-east-1:{ACCOUNT}:{}/L-00000000",
                quota.service_code
            ),
            "NoSuchResourceException",
        ),
        (
            "a service code the catalog does not hold",
            format!(
                "arn:aws:servicequotas:us-east-1:{ACCOUNT}:nosuchservice/{}",
                quota.quota_code
            ),
            "NoSuchResourceException",
        ),
        (
            "another account",
            format!(
                "arn:aws:servicequotas:us-east-1:123456789012:{}/{}",
                quota.service_code, quota.quota_code
            ),
            "NoSuchResourceException",
        ),
        (
            "a region this emulator does not serve",
            format!(
                "arn:aws:servicequotas:zz-nowhere-9:{ACCOUNT}:{}/{}",
                quota.service_code, quota.quota_code
            ),
            "NoSuchResourceException",
        ),
        (
            "another partition",
            format!(
                "arn:aws-us-gov:servicequotas:us-east-1:{ACCOUNT}:{}/{}",
                quota.service_code, quota.quota_code
            ),
            "NoSuchResourceException",
        ),
        (
            "a regional quota addressed with no region",
            format!(
                "arn:aws:servicequotas::{ACCOUNT}:{}/{}",
                quota.service_code, quota.quota_code
            ),
            "NoSuchResourceException",
        ),
        (
            // A GlobalQuota's reported ARN carries NO region, so a region segment
            // names a resource this service never reports.
            "a global quota addressed with a region",
            format!(
                "arn:aws:servicequotas:us-east-1:{ACCOUNT}:{}/{}",
                account_wide.service_code, account_wide.quota_code
            ),
            "NoSuchResourceException",
        ),
    ];

    for (case, arn, want) in rows {
        // The same ARN rule for all three tagging operations.
        let calls = vec![
            ("TagResource", tag_request(&arn, &[tag("k", "v")])),
            ("ListTagsForResource", json!({"ResourceARN": arn})),
            (
                "UntagResource",
                json!({"ResourceARN": arn, "TagKeys": ["k"]}),
            ),
        ];
        for (operation, body) in calls {
            let (status, code) = client.refused(operation, body).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR_STATUS, want),
                "case {case:?} on {operation} with {arn}: expected {CLIENT_ERROR_STATUS} {want}, \
                 got {status} {code}"
            );
        }
    }
}

#[tokio::test]
async fn a_tag_key_in_the_reserved_aws_namespace_is_refused() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let quota = regional(&catalog(&client).await);

    // (case, key, whether AWS reserves it)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("the reserved prefix with a name", "aws:cloudformation:stack-name", true),
        ("the reserved prefix alone", "aws:", true),
        ("the reserved prefix in capitals", "AWS:Foo", true),
        ("the reserved prefix mixed case", "Aws:Foo", true),
        // Only the PREFIX is reserved: a key that merely starts with the letters, or
        // carries them later, is the caller's own.
        ("a key beginning with the letters only", "awsome", false),
        ("the prefix later in the key", "my-aws:thing", false),
        ("a key that is a prefix of the reserved one", "aws", false),
    ];

    for (case, key, reserved) in rows {
        let body = tag_request(&quota.arn, &[tag(key, "v")]);
        if reserved {
            let (status, code) = client.refused("TagResource", body).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR_STATUS, "TagPolicyViolationException"),
                "case {case:?} ({key}): expected {CLIENT_ERROR_STATUS} TagPolicyViolationException, \
                 got {status} {code}"
            );
            let listed = client.tags(&quota.arn).await;
            assert!(
                !listed.contains_key(key),
                "case {case:?} ({key}): a refused tag must not be written, got {listed:?}"
            );
        } else {
            client.ok("TagResource", body).await;
            let listed = client.tags(&quota.arn).await;
            assert_eq!(
                listed.get(key).map(String::as_str),
                Some("v"),
                "case {case:?} ({key}): an unreserved key must be stored, got {listed:?}"
            );
            client.untag(&quota.arn, &[key.to_string()]).await;
        }
    }
}

#[tokio::test]
async fn tag_resource_requires_at_least_one_tag() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let quota = regional(&catalog(&client).await);

    // `InputTags` is len 1.. in the model, so an empty list is refused before the
    // handler runs — in this service's own `IllegalArgumentException`.
    let (status, code) = client
        .refused("TagResource", json!({"ResourceARN": quota.arn, "Tags": []}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR_STATUS, "IllegalArgumentException"),
        "case an empty Tags list: expected {CLIENT_ERROR_STATUS} IllegalArgumentException, \
         got {status} {code}"
    );
}
