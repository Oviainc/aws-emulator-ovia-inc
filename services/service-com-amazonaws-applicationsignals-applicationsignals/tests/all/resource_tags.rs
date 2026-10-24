// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of this service's tag reads and removals over HTTP:
//! `UntagResource` and the `ListTagsForResource` reader that reports what it left behind.
//!
//! The rule under test: a resource's tag set is a key-to-value map owned by the STORED
//! resource, `UntagResource` detaches exactly the keys the request names and nothing else,
//! and a `ResourceArn` that names no stored resource is refused with the modelled
//! `ResourceNotFoundException` without changing any tag. Every assertion below is made over
//! a SET of tag sets and key lists rather than on one request.
//!
//! The request helpers (`router`, `call`, …) are the ones `slo_lifecycle` already drives
//! this service's `restJson1` surface with, so every suite speaks to one harness.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{arn_uri_segment, call, create, router, sli_config, ACCOUNT, REGION};

/// The `ResourceType` every failure about an SLO reports: the resource's type NAME,
/// capitalised (`smithy_query(query="shape",shape="ResourceNotFoundException")` makes both
/// `ResourceType` and `ResourceId` `@required`).
const SLO_RESOURCE_TYPE: &str = "Slo";

/// The ARN of the SLO called `name` in this account and region, as the modelled
/// `ServiceLevelObjectiveArn` pattern spells it
/// (`^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`).
fn slo_arn(name: &str) -> String {
    format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{name}")
}

/// `GET /tags?ResourceArn=<arn>` — `ListTagsForResource` binds its only input member to a
/// query parameter (`smithy_query(query="operation",operation="ListTagsForResource")`).
///
/// `arn` is the already-escaped spelling `arn_uri_segment` produces, so the ARN's `:` and
/// `/` are carried as `%3A`/`%2F` rather than raw in the request line.
async fn list_tags_raw(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?ResourceArn={arn}"), None).await
}

/// [`list_tags_raw`] for the SLO called `name`.
async fn list_tags(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    list_tags_raw(router, &arn_uri_segment(&slo_arn(name))).await
}

/// `POST /untag-resource` with the given `ResourceArn` and `TagKeys`.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> (u16, Value) {
    call(
        router,
        "POST",
        "/untag-resource",
        Some(json!({ "ResourceArn": resource_arn, "TagKeys": keys })),
    )
    .await
}

/// The tags a `ListTagsForResource` response reports, as a key-to-value map.
///
/// `Tags` is a LIST of `Tag` structures on the wire; comparing it as a map is what makes the
/// assertions independent of the order the service happens to report them in.
fn reported_tags(body: &Value) -> BTreeMap<String, String> {
    body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `tags` as the modelled `TagList` a create request carries.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Create the SLO called `name` carrying `tags`, and assert the create succeeded.
async fn create_tagged(
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    tags: &[(String, String)],
) {
    let (status, created) = create(
        router,
        json!({
            "Name": name,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            "Tags": tag_list(tags),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the setup create of {name:?} to succeed, got {status} with {created}"
    );
}

/// An owned `(key, value)` pair, so a table row can hold both literals and generated keys.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One row of the removal table: the tags a resource is created with, the `TagKeys` the
/// untag request names, and the tags that must remain afterwards.
struct Case {
    label: &'static str,
    tags: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// The tag sets and key lists the rule is asserted over.
///
/// The rows differ along what the rule generalises over: how many tags the resource holds,
/// how many of them the request names, whether a named key is attached at all, whether a key
/// is named more than once, and the sizes the model admits for a key and a value. Keys and
/// values are drawn from the modelled constraints
/// (`smithy_query(query="shape",shape="Tag,TagKey,TagValue,TagKeyList")`): `TagKey` has
/// `length 1..128`, `TagValue` `length 0..256`, and `TagKeyList` `length 0..200` — so both
/// published key-length bounds and the empty key list appear below.
fn cases() -> Vec<Case> {
    // A resource holding many tags, to assert the removal is per key rather than wholesale.
    let many: Vec<(String, String)> = (0..50)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let many_removed: Vec<String> = many
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, (key, _))| key.clone())
        .collect();
    let many_kept: Vec<(String, String)> = many
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, pair)| pair.clone())
        .collect();

    vec![
        Case {
            label: "one key of three",
            tags: vec![tag("env", "prod"), tag("team", "payments"), tag("tier", "gold")],
            remove: vec!["team".to_string()],
            expected: vec![tag("env", "prod"), tag("tier", "gold")],
        },
        Case {
            label: "every key the resource carries",
            tags: vec![tag("env", "prod"), tag("team", "payments")],
            remove: vec!["env".to_string(), "team".to_string()],
            expected: vec![],
        },
        Case {
            // `TagKeyList` admits `length 0`, so an empty list names no key to detach.
            label: "no keys at all",
            tags: vec![tag("env", "prod"), tag("team", "payments")],
            remove: vec![],
            expected: vec![tag("env", "prod"), tag("team", "payments")],
        },
        Case {
            label: "a key the resource does not carry",
            tags: vec![tag("env", "prod")],
            remove: vec!["never-attached".to_string()],
            expected: vec![tag("env", "prod")],
        },
        Case {
            label: "an attached key beside an unattached one",
            tags: vec![tag("env", "prod"), tag("team", "payments")],
            remove: vec!["team".to_string(), "never-attached".to_string()],
            expected: vec![tag("env", "prod")],
        },
        Case {
            label: "the same key named twice in one request",
            tags: vec![tag("env", "prod"), tag("team", "payments")],
            remove: vec!["env".to_string(), "env".to_string()],
            expected: vec![tag("team", "payments")],
        },
        Case {
            // The shortest key the modelled `TagKey` `length 1..128` admits.
            label: "shortest key the model admits",
            tags: vec![tag("x", "1"), tag("y", "2")],
            remove: vec!["x".to_string()],
            expected: vec![tag("y", "2")],
        },
        Case {
            // The longest key the modelled `TagKey` `length 1..128` admits.
            label: "longest key the model admits",
            tags: vec![tag(&"k".repeat(128), "long"), tag("kept", "yes")],
            remove: vec!["k".repeat(128)],
            expected: vec![tag("kept", "yes")],
        },
        Case {
            // `TagValue` admits `length 0`: a tag with an empty value is a tag like any
            // other, removed by its key and kept when another key is removed.
            label: "a tag whose value is empty",
            tags: vec![tag("empty", ""), tag("team", "payments")],
            remove: vec!["team".to_string()],
            expected: vec![tag("empty", "")],
        },
        Case {
            label: "the empty-valued tag itself",
            tags: vec![tag("empty", ""), tag("team", "payments")],
            remove: vec!["empty".to_string()],
            expected: vec![tag("team", "payments")],
        },
        Case {
            label: "a resource holding no tags at all",
            tags: vec![],
            remove: vec!["env".to_string()],
            expected: vec![],
        },
        Case {
            label: "half of fifty keys",
            tags: many,
            remove: many_removed,
            expected: many_kept,
        },
    ]
}

/// `expected` as the map a `ListTagsForResource` response must report.
fn expected_map(expected: &[(String, String)]) -> BTreeMap<String, String> {
    expected.iter().cloned().collect()
}

/// An untag detaches exactly the keys the request names, leaves every other tag of the
/// resource with its value, and answers HTTP 200 with the empty body its modelled output
/// (no members) serialises to — for every shape of tag set and key list.
#[tokio::test]
async fn untag_detaches_exactly_the_named_keys() {
    for case in cases() {
        let router = router();
        let name = "tagged-slo";
        create_tagged(&router, case.label, name, &case.tags).await;

        // The reader reports what the create stored, so the removal is measured against a
        // known starting point rather than an assumed one.
        let (status, before) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the pre-untag list, got {status} with {before}",
            case.label
        );
        assert_eq!(
            reported_tags(&before),
            expected_map(&case.tags),
            "case {:?}: expected the created tags {:?}, got {:?}",
            case.label,
            expected_map(&case.tags),
            reported_tags(&before)
        );

        let (status, body) = untag(&router, &slo_arn(name), &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the untag, got {status} with {body}",
            case.label
        );
        assert_eq!(
            body,
            json!({}),
            "case {:?}: expected the untag to answer the empty body {{}}, got {body}",
            case.label
        );

        let (status, after) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the post-untag list, got {status} with {after}",
            case.label
        );
        assert_eq!(
            reported_tags(&after),
            expected_map(&case.expected),
            "case {:?}: expected the remaining tags {:?}, got {:?}",
            case.label,
            expected_map(&case.expected),
            reported_tags(&after)
        );

        // The resource itself survives: only its tags were addressed.
        let (status, slo) = call(&router, "GET", &format!("/slo/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the SLO to still be readable, got {status} with {slo}",
            case.label
        );
    }
}

/// Removal is per key, so repeating an untag changes nothing further: the second and third
/// calls answer exactly as the first did and the remaining tags stay put.
#[tokio::test]
async fn repeating_an_untag_changes_nothing_further() {
    for case in cases() {
        let router = router();
        let name = "repeat-slo";
        create_tagged(&router, case.label, name, &case.tags).await;

        for attempt in 1..=3 {
            let (status, body) = untag(&router, &slo_arn(name), &case.remove).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected HTTP 200, got {status} with {body}",
                case.label
            );

            let (status, after) = list_tags(&router, name).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected HTTP 200 from the list, got {status} with {after}",
                case.label
            );
            assert_eq!(
                reported_tags(&after),
                expected_map(&case.expected),
                "case {:?} attempt {attempt}: expected the remaining tags {:?}, got {:?}",
                case.label,
                expected_map(&case.expected),
                reported_tags(&after)
            );
        }
    }
}

/// A tag set belongs to ONE resource: untagging one resource leaves every other resource's
/// identically-keyed tags exactly as they were.
#[tokio::test]
async fn untagging_one_resource_leaves_another_resources_tags_alone() {
    let router = router();
    let shared = vec![tag("env", "prod"), tag("team", "payments")];
    for name in ["first-slo", "second-slo", "third-slo"] {
        create_tagged(&router, "two resources", name, &shared).await;
    }

    let (status, body) = untag(
        &router,
        &slo_arn("second-slo"),
        &["env".to_string(), "team".to_string()],
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200 from the untag, got {status} with {body}");

    for name in ["first-slo", "third-slo"] {
        let (status, after) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the list, got {status} with {after}"
        );
        assert_eq!(
            reported_tags(&after),
            expected_map(&shared),
            "case {name:?}: expected an untouched resource to keep {:?}, got {:?}",
            expected_map(&shared),
            reported_tags(&after)
        );
    }

    let (status, after) = list_tags(&router, "second-slo").await;
    assert_eq!(
        status, 200,
        "case second-slo: expected HTTP 200 from the list, got {status} with {after}"
    );
    assert!(
        reported_tags(&after).is_empty(),
        "case second-slo: expected the untagged resource to carry no tags, got {:?}",
        reported_tags(&after)
    );
}

/// A `ResourceArn` that names no resource this service holds is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404) — on the removal and on the read alike — and the
/// refused removal changes no tag.
///
/// The rows differ along WHY the ARN names nothing: a resource that was never created, one
/// created and then deleted, a bare resource name (which is not an ARN at all, and
/// `ResourceArn` is modelled as an `AmazonResourceName`), an ARN of another service's
/// resource type, and an ARN whose resource segment is not this service's.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_and_changes_nothing() {
    let router = router();
    let kept = vec![tag("env", "prod"), tag("team", "payments")];
    create_tagged(&router, "missing resource", "live-slo", &kept).await;

    create_tagged(&router, "missing resource", "gone-slo", &kept).await;
    let (status, deleted) = call(&router, "DELETE", "/slo/gone-slo", None).await;
    assert_eq!(
        status, 200,
        "expected the setup delete to succeed, got {status} with {deleted}"
    );

    let rows = [
        ("never created", slo_arn("absent-slo")),
        ("created then deleted", slo_arn("gone-slo")),
        ("a bare resource name, not an ARN", "live-slo".to_string()),
        (
            "another service's resource",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm/my-alarm"),
        ),
        (
            "an ARN with no slo resource segment",
            format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:service/checkout"),
        ),
        (
            // Another service's ARN that merely CARRIES an SLO-shaped resource field: the
            // service field is what says whose resource an ARN names.
            "another service's ARN with an slo-shaped resource field",
            "arn:aws:s3:::slo/live-slo".to_string(),
        ),
        (
            // This store holds ONE account's resources, so another account's ARN names none
            // of them even when the name matches.
            "another account's ARN for the same name",
            format!("arn:aws:application-signals:{REGION}:123456789012:slo/live-slo"),
        ),
        (
            // Not an ARN at all: too few fields to carry a resource.
            "an arn-prefixed string with too few fields",
            "arn:aws:application-signals:slo/live-slo".to_string(),
        ),
    ];

    for (label, resource_arn) in &rows {
        let (status, error) = untag(&router, resource_arn, &["env".to_string()]).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 for an ARN naming no resource, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {error}"
        );
        assert_eq!(
            error["ResourceType"].as_str(),
            Some(SLO_RESOURCE_TYPE),
            "case {label:?}: expected ResourceType {SLO_RESOURCE_TYPE:?}, got {error}"
        );
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(resource_arn.as_str()),
            "case {label:?}: expected the failure to identify the resource by the ARN the request named ({resource_arn:?}), got {error}"
        );

        // The reader refuses the same ARN identically: both operations address a resource.
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(resource_arn)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 from the read, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected the read to answer a ResourceNotFoundException, got {error}"
        );
    }

    // Nothing the refusals touched: the one stored resource still carries every tag.
    let (status, after) = list_tags(&router, "live-slo").await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the list, got {status} with {after}"
    );
    assert_eq!(
        reported_tags(&after),
        expected_map(&kept),
        "expected a refused untag to change nothing, got {:?}",
        reported_tags(&after)
    );
}

/// A `TagKeys` entry outside the modelled `TagKey` `length 1..128`, or a list longer than
/// `TagKeyList`'s `length 0..200`, is no key list the API accepts: the request is refused
/// with a 400 `ValidationException` and detaches nothing. Each published bound is asserted
/// at the bound (accepted) and one past it (refused).
#[tokio::test]
async fn a_key_list_outside_the_modelled_bounds_is_refused_and_detaches_nothing() {
    let router = router();
    let name = "bounded-slo";
    let kept = vec![tag("env", "prod"), tag("k".repeat(128).as_str(), "longest")];
    create_tagged(&router, "bounds", name, &kept).await;

    // One past each published bound: a key of 129 characters, a key of 0 characters, and a
    // list of 201 keys.
    let refused: [(&str, Vec<String>); 3] = [
        ("key one character past the maximum", vec!["k".repeat(129)]),
        ("empty key, one below the minimum", vec![String::new()]),
        (
            "201 keys, one past the list maximum",
            (0..201).map(|index| format!("key-{index:03}")).collect(),
        ),
    ];

    for (label, keys) in &refused {
        let (status, error) = untag(&router, &slo_arn(name), keys).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a key list the model refuses, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );

        let (_status, after) = list_tags(&router, name).await;
        assert_eq!(
            reported_tags(&after),
            expected_map(&kept),
            "case {label:?}: expected a refused untag to detach nothing, got {:?}",
            reported_tags(&after)
        );
    }

    // AT each published bound the request is accepted: a 128-character key (the longest the
    // model admits) inside a list of exactly 200 keys.
    let mut at_bound: Vec<String> = (0..199).map(|index| format!("key-{index:03}")).collect();
    at_bound.push("k".repeat(128));
    let (status, body) = untag(&router, &slo_arn(name), &at_bound).await;
    assert_eq!(
        status, 200,
        "case at the published bounds: expected HTTP 200, got {status} with {body}"
    );
    let (_status, after) = list_tags(&router, name).await;
    assert_eq!(
        reported_tags(&after),
        expected_map(&[tag("env", "prod")]),
        "case at the published bounds: expected only the unnamed tag to remain, got {:?}",
        reported_tags(&after)
    );
}

/// An ARN of this service and account names the stored resource whatever REGION it carries:
/// this process holds one account's and one Region's records, and the region a minted ARN
/// carries is a rendering of the writer's SigV4 credential scope rather than part of the
/// resource's identity (the rule `src/state.rs` states for every ARN this service resolves).
#[tokio::test]
async fn an_arn_resolves_whatever_region_it_carries() {
    let router = router();
    let name = "region-slo";
    let tags = vec![tag("env", "prod"), tag("team", "payments")];
    create_tagged(&router, "region", name, &tags).await;

    // Regions other than the one the setup's credential scope named.
    for region in ["us-east-1", "ap-southeast-2"] {
        let arn = format!("arn:aws:application-signals:{region}:{ACCOUNT}:slo/{name}");
        let (status, body) = untag(&router, &arn, &[]).await;
        assert_eq!(
            status, 200,
            "case region {region:?}: expected HTTP 200 for an ARN of this account, got {status} with {body}"
        );
    }

    let (_status, after) = list_tags(&router, name).await;
    assert_eq!(
        reported_tags(&after),
        expected_map(&tags),
        "expected the empty key lists to detach nothing, got {:?}",
        reported_tags(&after)
    );
}

/// The read reports, and never writes: repeated `ListTagsForResource` calls answer the same
/// tags, and a read between two untags does not change what the second untag finds.
#[tokio::test]
async fn reading_the_tags_changes_nothing() {
    let router = router();
    let name = "read-slo";
    let tags = vec![tag("env", "prod"), tag("team", "payments"), tag("tier", "gold")];
    create_tagged(&router, "read", name, &tags).await;

    let (_status, first) = list_tags(&router, name).await;
    for attempt in 1..=3 {
        let (status, again) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case attempt {attempt}: expected HTTP 200 from the list, got {status} with {again}"
        );
        assert_eq!(
            reported_tags(&again),
            reported_tags(&first),
            "case attempt {attempt}: expected the read to report {:?}, got {:?}",
            reported_tags(&first),
            reported_tags(&again)
        );
    }

    let (status, body) = untag(&router, &slo_arn(name), &["env".to_string()]).await;
    assert_eq!(status, 200, "expected HTTP 200 from the untag, got {status} with {body}");
    let (_status, after) = list_tags(&router, name).await;
    assert_eq!(
        reported_tags(&after),
        expected_map(&[tag("team", "payments"), tag("tier", "gold")]),
        "expected the surviving tags after one removal, got {:?}",
        reported_tags(&after)
    );
}
