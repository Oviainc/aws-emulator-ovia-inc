// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures. What was removed is read
//! back through `ListTagsForResource`, the operation that reports the same tag store.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * untagging DROPS exactly the keys the request names and leaves every other key of the
//!   resource's tag set exactly as it was, for any resource this service mints an ARN for
//!   and any key list inside `TagKeys`' bounds;
//! * a key the resource does not carry has nothing to remove, so the call is idempotent:
//!   repeating it answers the same way and changes nothing more, and a key that differs
//!   from an attached one only in letter case is a DIFFERENT key;
//! * one resource's tags are its own: untagging one never changes another's;
//! * an `Arn` that names no resource of this service is `ResourceNotFoundException` (404)
//!   and removes nothing;
//! * a key in the reserved namespace (`state::RESERVED_TAG_KEY_PREFIX`) names a tag Amazon
//!   Web Services applies itself, which a caller may not delete: the call is refused with
//!   the modelled `ValidationException` (400) and removes nothing at all.
//!
//! Every bound written out by hand names its source. `UntagResource` binds
//! `@http DELETE /tags/{resourceArn} -> 204` with `resourceArn` an `Arn`
//! (`@length(20..2048)`, `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`, `@httpLabel`) and
//! `tagKeys` a `TagKeys` (`@length(0..200)` of `TagKey`, `@length(1..128)`) bound as
//! `@httpQuery("tagKeys")`, with the errors `InternalServerException` (500),
//! `ResourceNotFoundException` (404) and `ValidationException` (400) — from
//! `smithy_query(query="operation", operation="UntagResource")` and
//! `smithy_query(query="shape", shape="TagKeys,TagKey,Arn")` on this crate's
//! `smithy-model/`.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{call, dispatch, emulator, past_the_settle_span, Answer};
use crate::reserved_tag_keys::reserved_keys;
use crate::resource_tags::{apply_tags, read_tags, reported_tags, resources, tags, tags_path};

/// `UntagResource`: DELETE the tags `keys` names from `arn`.
///
/// `tagKeys` is `@httpQuery`, so the keys travel as one REPEATED query parameter, which is
/// how a list binds to a query string. Every key the tables below send is drawn from the
/// unreserved ASCII set plus `:`, all of which a query component carries literally, so the
/// value the front door decodes is the value written here; `query_safe` is where that
/// property is stated for the keys a table does not spell out itself.
async fn remove_tags(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| {
            assert!(
                query_safe(key),
                "key {key:?}: this suite sends tag keys literally in the query string, so a \
                 row must use one a query component carries as written"
            );
            format!("tagKeys={key}")
        })
        .collect::<Vec<String>>()
        .join("&");
    dispatch(router, "DELETE", &format!("{}?{query}", tags_path(arn)), None).await
}

/// Whether `key` survives a query component unchanged: ASCII, and free of the characters a
/// query string gives a meaning of its own (`&` and `=` separate, `+` decodes to a space,
/// `%` opens an escape, and a space or control character cannot travel in a URI at all).
fn query_safe(key: &str) -> bool {
    key.chars()
        .all(|c| c.is_ascii() && !c.is_ascii_whitespace() && !c.is_ascii_control())
        && !key.contains(['&', '=', '+', '%', '#', '?'])
}

/// Assert `answer` is the 204 the model's `@http` trait gives this operation, carrying no
/// members.
fn assert_removed(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: expected 204, got {} body {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "case {case:?}: a 204 answer carries no members, got {:?}",
        answer.body
    );
}

/// The tag set every removal row starts from, re-applied before each row so one row's
/// removals cannot be read as the next one's.
///
/// `TagKey` is `@length(1..128)` and `TagValue` is `@length(0..256)`, so the set spans
/// both key length bounds, an empty value, and two keys that differ only in letter case.
fn base_tags() -> BTreeMap<String, String> {
    let longest_key = "k".repeat(128);
    tags(&[
        ("K", ""),
        ("Owner", "platform"),
        ("owner", "lower-case"),
        ("Team", "governance"),
        ("Env", "prod"),
        (longest_key.as_str(), "at-the-key-length-max"),
    ])
}

/// One row of the removal table: the keys one `UntagResource` call names.
struct Removal {
    case: &'static str,
    keys: Vec<String>,
}

/// The removal table. Rows differ along what the rule generalises over: the number of keys
/// (one, several, all of them), whether a named key is attached at all, the key's length at
/// `TagKey`'s published bounds of 1 and 128, a repeated key, and a key differing from an
/// attached one only in letter case.
fn removals() -> Vec<Removal> {
    let longest_key = "k".repeat(128);
    let mut all: Vec<String> = base_tags().keys().cloned().collect();
    all.sort();
    vec![
        Removal {
            case: "one attached key",
            keys: vec!["Team".to_string()],
        },
        Removal {
            case: "several attached keys at once",
            keys: vec!["Owner".to_string(), "Env".to_string()],
        },
        Removal {
            case: "a key the resource does not carry",
            keys: vec!["NeverAttached".to_string()],
        },
        Removal {
            case: "an attached key beside one that is not attached",
            keys: vec!["Env".to_string(), "NeverAttached".to_string()],
        },
        Removal {
            case: "the attached key at TagKey's length minimum of 1",
            keys: vec!["K".to_string()],
        },
        Removal {
            case: "the attached key at TagKey's length maximum of 128",
            keys: vec![longest_key.clone()],
        },
        Removal {
            case: "a key that is not attached, at TagKey's length maximum of 128",
            keys: vec!["n".repeat(128)],
        },
        Removal {
            case: "the same key named twice in one call",
            keys: vec!["Team".to_string(), "Team".to_string()],
        },
        Removal {
            case: "a key differing from an attached one only in letter case",
            keys: vec!["OWNER".to_string()],
        },
        Removal {
            case: "every key the resource carries",
            keys: all,
        },
    ]
}

/// RULE: untagging drops exactly the keys the request names, leaves every other key as it
/// was, and is idempotent — for every resource kind this service tags and every key list
/// inside `TagKeys`' bounds. One resource's tags stay its own.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let router = emulator();
    let resources = resources(&router).await;

    for row in removals() {
        let borrowed: Vec<&str> = row.keys.iter().map(String::as_str).collect();

        for resource in &resources {
            // Every row starts from the same known set: a merge overwrites the keys it
            // carries and adds the ones the previous row removed, so the resource carries
            // exactly `base_tags()` again.
            let restored = apply_tags(&router, &resource.arn, &base_tags()).await;
            assert_eq!(
                restored.status, 204,
                "case {:?} on {:?}: restoring the base tag set expected 204, got {} body \
                 {:?}",
                row.case, resource.case, restored.status, restored.body
            );
            let before = reported_tags(&read_tags(&router, &resource.arn).await);
            assert_eq!(
                before,
                base_tags(),
                "case {:?} on {:?}: expected the base tag set {:?}, got {before:?}",
                row.case,
                resource.case,
                base_tags()
            );

            let mut expected = before.clone();
            for key in &row.keys {
                expected.remove(key);
            }

            let removed = remove_tags(&router, &resource.arn, &borrowed).await;
            assert_removed(&format!("{} on {}", row.case, resource.case), &removed);

            let read = read_tags(&router, &resource.arn).await;
            assert_eq!(
                read.status, 200,
                "case {:?} on {:?}: expected the tags to stay readable with 200, got {} \
                 body {:?}",
                row.case, resource.case, read.status, read.body
            );
            assert_eq!(
                reported_tags(&read),
                expected,
                "case {:?} on {:?}: expected the tag set {expected:?}, got {:?}",
                row.case,
                resource.case,
                reported_tags(&read)
            );

            // Idempotence: the same call again answers the same way and changes nothing,
            // because the keys it names are no longer there to remove.
            let again = remove_tags(&router, &resource.arn, &borrowed).await;
            assert_removed(
                &format!("{} on {}, repeated", row.case, resource.case),
                &again,
            );
            assert_eq!(
                reported_tags(&read_tags(&router, &resource.arn).await),
                expected,
                "case {:?} on {:?}: repeating the removal must change nothing, expected \
                 {expected:?}",
                row.case,
                resource.case
            );
        }
    }
}

/// RULE: untagging one resource never changes another's tag set, asserted directly — each
/// resource carries a DIFFERENT value under the same key, and after one is untagged the
/// others report exactly what they carried.
#[tokio::test]
async fn untagging_one_resource_leaves_the_others_alone() {
    let router = emulator();
    let resources = resources(&router).await;

    let mut expected: Vec<BTreeMap<String, String>> = Vec::new();
    for (index, resource) in resources.iter().enumerate() {
        let own = tags(&[("Shared", "value"), ("Own", "x")]);
        let applied = apply_tags(&router, &resource.arn, &own).await;
        assert_eq!(
            applied.status, 204,
            "case {:?}: expected 204, got {} body {:?}",
            resource.case, applied.status, applied.body
        );
        let mut set = reported_tags(&read_tags(&router, &resource.arn).await);
        assert!(
            set.contains_key("Shared"),
            "case {:?}: expected the applied tags, got {set:?}",
            resource.case
        );
        if index == 0 {
            set.remove("Shared");
        }
        expected.push(set);
    }

    let subject = &resources[0];
    let removed = remove_tags(&router, &subject.arn, &["Shared"]).await;
    assert_removed("the shared key on the first resource", &removed);

    for (index, resource) in resources.iter().enumerate() {
        let read = read_tags(&router, &resource.arn).await;
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, expected[index].clone()),
            "case {:?}: expected {:?}, got {} body {:?}",
            resource.case,
            expected[index],
            read.status,
            read.body
        );
    }
}

/// RULE: an `Arn` that names no resource of this service is `ResourceNotFoundException`
/// (404), and such a refusal removes nothing from the resources that DO exist.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_not_found() {
    let router = emulator();
    let resources = resources(&router).await;
    for resource in &resources {
        let applied = apply_tags(&router, &resource.arn, &base_tags()).await;
        assert_eq!(
            applied.status, 204,
            "case {:?}: expected 204, got {} body {:?}",
            resource.case, applied.status, applied.body
        );
    }

    // A resource whose ARN this service DID mint and then tore down: its tags went with
    // it, so the ARN names nothing again. The create must be due before the delete is
    // accepted, and the delete due before the landing zone is gone.
    tokio::time::sleep(past_the_settle_span()).await;
    let deleted = call(
        &router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": resources[0].arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "delete-landingzone: expected 200, got {} body {:?}",
        deleted.status, deleted.body
    );
    tokio::time::sleep(past_the_settle_span()).await;

    // Every value satisfies `Arn`'s @length(20..2048) and
    // @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$, so the front door passes it and the handler is
    // what refuses it.
    let cases: Vec<(&str, String)> = vec![
        (
            "a landing zone ARN nothing minted",
            "arn:aws:controltower:us-west-2:000000000000:landingzone/\
             00000000-0000-0000-0000-000000000000"
                .to_string(),
        ),
        (
            "an enabled control ARN nothing minted",
            "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/\
             deadbeef-dead-beef-dead-beefdeadbeef"
                .to_string(),
        ),
        (
            "an enabled baseline ARN nothing minted",
            "arn:aws:controltower:us-west-2:000000000000:enabledbaseline/\
             deadbeef-dead-beef-dead-beefdeadbeef"
                .to_string(),
        ),
        (
            "an ARN of exactly the length min of 20",
            format!("arn:aws{}", "a".repeat(13)),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::emulator-tagging-bucket".to_string(),
        ),
        (
            "the landing zone's own ARN, after it was deleted",
            resources[0].arn.clone(),
        ),
    ];

    for (case, arn) in cases {
        let refused = remove_tags(&router, &arn, &["Owner"]).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.string("message").contains(&arn),
            "case {case:?}: expected the refusal to name the ARN it could not resolve, got \
             {:?}",
            refused.string("message")
        );
    }

    // The resources that DO exist still carry every tag: a refusal about another ARN
    // removes nothing.
    for resource in &resources[1..] {
        let read = read_tags(&router, &resource.arn).await;
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, base_tags()),
            "case {:?}: a refused untagging of another ARN must remove nothing, got {} \
             body {:?}",
            resource.case,
            read.status,
            read.body
        );
    }
}

/// RULE: a key in the reserved namespace names a tag Amazon Web Services applies itself,
/// which a caller may not delete — the call is refused with 400 `ValidationException`, in
/// every form the prefix takes, and removes NOTHING, not even the ordinary keys named
/// beside it.
#[tokio::test]
async fn untagging_a_reserved_tag_key_is_refused_and_removes_nothing() {
    let router = emulator();
    let resources = resources(&router).await;

    for resource in &resources {
        let applied = apply_tags(&router, &resource.arn, &base_tags()).await;
        assert_eq!(
            applied.status, 204,
            "case {:?}: expected 204, got {} body {:?}",
            resource.case, applied.status, applied.body
        );

        // The shared reserved-key table, minus the row whose remainder is not ASCII: a
        // query component carries ASCII, so that form reaches a handler only through the
        // write path, where `reserved_tag_keys` already drives it. The prefix rule is the
        // same either way — the rows kept here span the prefix alone, the AWS-generated
        // keys, its other letter cases and a key at `TagKey`'s length maximum.
        for (case, key) in reserved_keys()
            .into_iter()
            .filter(|(_, key)| query_safe(key))
        {
            // The reserved key alone, then beside ordinary keys the resource DOES carry,
            // so the refusal is for the whole key list rather than for one of its values.
            let rows: Vec<(String, Vec<&str>)> = vec![
                (format!("{case} alone"), vec![key.as_str()]),
                (
                    format!("{case} before an attached key"),
                    vec![key.as_str(), "Team"],
                ),
                (
                    format!("{case} after an attached key"),
                    vec!["Owner", key.as_str()],
                ),
            ];

            for (row, keys) in rows {
                let refused = remove_tags(&router, &resource.arn, &keys).await;
                assert_eq!(
                    (refused.status, refused.error_code.as_deref()),
                    (400, Some("ValidationException")),
                    "case {row:?} on {:?}: expected 400 ValidationException, got {:?} body \
                     {:?}",
                    resource.case,
                    (refused.status, refused.error_code.as_deref()),
                    refused.body
                );
                assert!(
                    refused.string("message").contains(&key),
                    "case {row:?}: expected the refusal to name the offending key {key:?}, \
                     got {:?}",
                    refused.string("message")
                );

                let read = read_tags(&router, &resource.arn).await;
                assert_eq!(
                    reported_tags(&read),
                    base_tags(),
                    "case {row:?} on {:?}: a refused untagging must remove nothing, got \
                     {:?}",
                    resource.case,
                    reported_tags(&read)
                );
            }
        }
    }
}
