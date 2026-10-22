// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for EFS `DescribeTags`: the tag set reported for a file system is
//! the one that file system holds in state, read back over HTTP through the same
//! `MultiServiceRouter` the production entrypoint serves.
//!
//! The rule is asserted over a TABLE of inputs rather than on one call: the rows vary
//! the tag count (none, one, a few, the published 50-per-file-system quota), the key and
//! value forms the modelled `TagKey`/`TagValue` patterns allow, and the form of the id
//! (`fs-…` or a file-system ARN). Two further tables vary `MaxItems` — which this
//! operation documents it ignores — and `Marker`, over two tag-set sizes.
//!
//! The harness (`Emulator`, `Reply`) is the one `file_system_lifecycle.rs` already
//! drives this service with; only the `DescribeTags` binding is added here.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::file_system_lifecycle::{Emulator, Reply};

/// `GET /2015-02-01/tags/{FileSystemId}` — the `DescribeTags` HTTP binding
/// (`smithy_query(query="operation",operation="DescribeTags")`).
async fn describe_tags(efs: &Emulator, file_system_id: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        format!("/2015-02-01/tags/{file_system_id}")
    } else {
        format!("/2015-02-01/tags/{file_system_id}?{query}")
    };
    efs.send("GET", &path, None).await
}

/// A `(Key, Value)` table as the modelled `Tags` list.
fn tags_json(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The `Tags` of a reply as `(Key, Value)` pairs, in the order the service reported.
fn reported(reply: &Reply) -> Vec<(String, String)> {
    reply
        .get("Tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The same pairs ordered by key, so a set is compared against a set: "The order of tags
/// returned in the response of one DescribeTags call … is unspecified", so no assertion
/// about the whole set may depend on that order.
fn as_set(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut sorted = pairs.to_vec();
    sorted.sort();
    sorted
}

/// `index` distinct tags, which is what the size-varying rows are built from.
fn numbered_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
        .collect()
}

/// One row of the tag-set table: the `Tags` a file system is created with.
struct TagSetCase {
    case: &'static str,
    tags: Vec<(String, String)>,
}

/// The tag sets the reporting rule is asserted over. The key and value forms come from
/// the modelled patterns (`smithy_query(query="shape",shape="TagKey,TagValue")`):
/// `TagKey` is 1..128 of `[\p{L}\p{Z}\p{N}_.:/=+\-@]`, `TagValue` the same class at
/// 0..256 — so an empty value and a non-ASCII letter are both in the class, and the
/// 50-tag row is the published quota [`state::MAX_TAGS_PER_RESOURCE`].
fn tag_set_table() -> Vec<TagSetCase> {
    let owned = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };
    vec![
        TagSetCase {
            case: "no tags at all",
            tags: Vec::new(),
        },
        TagSetCase {
            case: "the Name tag alone",
            tags: owned(&[("Name", "demo-fs")]),
        },
        TagSetCase {
            case: "three tags",
            tags: owned(&[
                ("Name", "analytics"),
                ("env", "prod"),
                ("owner", "data-platform"),
            ]),
        },
        TagSetCase {
            case: "key and value forms the modelled patterns allow",
            tags: owned(&[
                ("cost:center/eu", "team=platform+core"),
                ("Équipe.de_test", "valeur naïve"),
                ("owner@example.com", "on-call"),
                ("empty-value", ""),
            ]),
        },
        TagSetCase {
            case: "the 50-tag per-file-system quota",
            tags: numbered_tags(state::MAX_TAGS_PER_RESOURCE),
        },
    ]
}

#[tokio::test]
async fn describe_tags_reports_the_tag_set_each_file_system_holds() {
    let efs = Emulator::new();
    let table = tag_set_table();

    // Every row's file system lives in the SAME emulator, so the answer must also be
    // keyed by file system: one file system's tags may not appear under another's id.
    let mut created = Vec::new();
    for (index, row) in table.iter().enumerate() {
        let reply = efs
            .create_file_system(json!({
                "CreationToken": format!("tags-{index}"),
                "Tags": tags_json(&row.tags),
            }))
            .await;
        assert_eq!(
            reply.status,
            201,
            "case {:?}: CreateFileSystem expected 201, got {} {:?}",
            row.case,
            reply.status,
            reply.body
        );
        created.push((reply.string("FileSystemId"), reply.string("FileSystemArn")));
    }

    for (row, (id, arn)) in table.iter().zip(&created) {
        // "The ID of the file system whose tag set you want to retrieve" — the member's
        // pattern accepts a bare id OR a file-system ARN, so both name the same tag set.
        // The ARN's `/` is percent-encoded, as an SDK encodes a non-greedy path label.
        for form in [id.clone(), arn.replace('/', "%2F")] {
            let reply = describe_tags(&efs, &form, "").await;
            assert_eq!(
                reply.status,
                200,
                "case {:?} as {form:?}: expected 200, got {} {:?}",
                row.case,
                reply.status,
                reply.body
            );
            assert_eq!(
                as_set(&reported(&reply)),
                as_set(&row.tags),
                "case {:?} as {form:?}: unexpected tag set, got {:?}",
                row.case,
                reply.get("Tags")
            );
            assert!(
                reply.get("Marker").is_null(),
                "case {:?} as {form:?}: no Marker was requested, so none is echoed, got {:?}",
                row.case,
                reply.get("Marker")
            );
            assert!(
                reply.get("NextMarker").is_null(),
                "case {:?} as {form:?}: {} tags fit one page, so there is no NextMarker, got {:?}",
                row.case,
                row.tags.len(),
                reply.get("NextMarker")
            );
        }
    }
}

#[tokio::test]
async fn max_items_does_not_change_the_reported_tag_set() {
    // `DescribeTags$MaxItems`: "(Optional) The maximum number of file system tags to
    // return in the response. Currently, this number is automatically set to 100, and
    // other values are ignored."
    let efs = Emulator::new();
    let tags = numbered_tags(state::MAX_TAGS_PER_RESOURCE);
    let created = efs
        .create_file_system(json!({
            "CreationToken": "max-items",
            "Tags": tags_json(&tags),
        }))
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status,
        created.body
    );
    let id = created.string("FileSystemId");

    let baseline = as_set(&reported(&describe_tags(&efs, &id, "").await));
    assert_eq!(
        baseline.len(),
        tags.len(),
        "the baseline answer must hold every stored tag, got {baseline:?}"
    );
    // 1 is the modelled lower bound (`range {min: 1}`); 100 is the page the operation
    // documents, and the rows either side of it straddle that boundary.
    for max_items in [1, 2, 49, 50, 99, 100, 101, 1000, i32::MAX] {
        let reply = describe_tags(&efs, &id, &format!("MaxItems={max_items}")).await;
        assert_eq!(
            reply.status,
            200,
            "case MaxItems={max_items}: expected 200, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            as_set(&reported(&reply)),
            baseline,
            "case MaxItems={max_items}: MaxItems is ignored, so the whole tag set is \
             returned, got {:?}",
            reply.get("Tags")
        );
        assert!(
            reply.get("NextMarker").is_null(),
            "case MaxItems={max_items}: the page held the whole set, so there is no \
             NextMarker, got {:?}",
            reply.get("NextMarker")
        );
    }
}

#[tokio::test]
async fn marker_continues_the_tag_set_where_the_previous_call_left_off() {
    let efs = Emulator::new();
    for count in [3usize, state::MAX_TAGS_PER_RESOURCE] {
        let tags = numbered_tags(count);
        let created = efs
            .create_file_system(json!({
                "CreationToken": format!("marker-{count}"),
                "Tags": tags_json(&tags),
            }))
            .await;
        assert_eq!(
            created.status,
            201,
            "case {count} tags: CreateFileSystem expected 201, got {} {:?}",
            created.status,
            created.body
        );
        let id = created.string("FileSystemId");

        // The order tags come back in is unspecified, so the expectation is taken from
        // the service's OWN first answer: continuing at the key of entry `index` must
        // report exactly that answer's entries from `index` on.
        let whole = reported(&describe_tags(&efs, &id, "").await);
        assert_eq!(
            whole.len(),
            count,
            "case {count} tags: expected the whole set, got {whole:?}"
        );
        for (index, (key, _)) in whole.iter().enumerate() {
            let reply = describe_tags(&efs, &id, &format!("Marker={key}")).await;
            assert_eq!(
                reply.status,
                200,
                "case {count} tags, Marker={key}: expected 200, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reported(&reply),
                whole[index..].to_vec(),
                "case {count} tags, Marker={key}: expected the entries from {index} on, \
                 got {:?}",
                reply.get("Tags")
            );
            assert_eq!(
                reply.string("Marker"),
                *key,
                "case {count} tags, Marker={key}: the request's Marker is echoed, got {:?}",
                reply.get("Marker")
            );
            assert!(
                reply.get("NextMarker").is_null(),
                "case {count} tags, Marker={key}: the rest of the set fit one page, so \
                 there is no NextMarker, got {:?}",
                reply.get("NextMarker")
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_file_system_or_marker_is_refused_rather_than_answered_empty() {
    let efs = Emulator::new();
    let created = efs
        .create_file_system(json!({
            "CreationToken": "refusals",
            "Tags": tags_json(&numbered_tags(3)),
        }))
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status,
        created.body
    );
    let id = created.string("FileSystemId");

    // Ids that satisfy the modelled pattern (`fs-` then 8..40 hex digits, here at both
    // published bounds) but name no file system: the answer is the modelled 404, never
    // an empty tag list.
    for unknown in [
        "fs-00000000",
        "fs-deadbeef",
        "fs-0123456789abcdef0123456789abcdef01234567",
    ] {
        let reply = describe_tags(&efs, unknown, "").await;
        assert_eq!(
            reply.status,
            404,
            "case {unknown:?}: expected 404, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("FileSystemNotFound"),
            "case {unknown:?}: unexpected error type, got {:?}",
            reply.error_type
        );
    }

    // A Marker is "an opaque pagination token returned from a previous DescribeTags
    // operation", so one that names no entry of this tag set cannot have come from one.
    for marker in ["not-a-tag-key", "key-99", "0"] {
        let reply = describe_tags(&efs, &id, &format!("Marker={marker}")).await;
        assert_eq!(
            reply.status,
            400,
            "case Marker={marker:?}: expected 400, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequest"),
            "case Marker={marker:?}: unexpected error type, got {:?}",
            reply.error_type
        );
    }
}

#[tokio::test]
async fn describing_tags_does_not_change_what_a_later_caller_sees() {
    let efs = Emulator::new();
    let tags = numbered_tags(4);
    let created = efs
        .create_file_system(json!({
            "CreationToken": "pure-read",
            "Tags": tags_json(&tags),
        }))
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status,
        created.body
    );
    let id = created.string("FileSystemId");

    let first = describe_tags(&efs, &id, "").await;
    // Every read form, including the paginating ones, before reading again.
    for query in ["MaxItems=1", "Marker=key-02", "MaxItems=100&Marker=key-00"] {
        let reply = describe_tags(&efs, &id, query).await;
        assert_eq!(
            reply.status,
            200,
            "case {query:?}: expected 200, got {} {:?}",
            reply.status,
            reply.body
        );
    }
    let again = describe_tags(&efs, &id, "").await;
    assert_eq!(
        reported(&again),
        reported(&first),
        "a DescribeTags call must not change the tag set a later caller reads, got {:?}",
        again.get("Tags")
    );

    // Nor is the file system itself touched by the reads.
    let described = efs
        .send(
            "GET",
            &format!("/2015-02-01/file-systems?FileSystemId={id}"),
            None,
        )
        .await;
    assert_eq!(
        described.status,
        200,
        "DescribeFileSystems: expected 200, got {} {:?}",
        described.status,
        described.body
    );
    let file_systems = described
        .get("FileSystems")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let still_held: Vec<(String, String)> = file_systems
        .first()
        .and_then(|file_system| file_system["Tags"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        as_set(&still_held),
        as_set(&tags),
        "DescribeFileSystems must still report the stored tag set, got {file_systems:?}"
    );
}
