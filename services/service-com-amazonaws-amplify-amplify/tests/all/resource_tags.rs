// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of Amplify resource tagging: `TagResource` and the read
//! each write is proved through, `ListTagsForResource`.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses, not an internal function's return value.
//!
//! The rule under test: a tag write MERGES the request's tag map into the map persisted
//! on the resource its ARN names — a key the resource already carries takes the new
//! value, every other key it carries is left alone — and every read of that resource
//! reports the merged map, whether it is asked for as a tag listing or as the resource
//! record itself. An ARN naming no resource of a kind this service persists tags for is
//! refused 404, and a write that would leave the resource past the modelled `TagMap`
//! bound is refused 400 and stores nothing.
//!
//! The tables span the class the rule generalises over — the two resource kinds the
//! model gives a tag map (an app and a branch, the branch also under a name carrying
//! the slash a Git branch name commonly has), tags set at create time and tags set
//! afterwards, disjoint and colliding keys, one write and several, the empty map, a key
//! and a value at their modelled `length` maxima, and the tag count at the modelled
//! limit and one past it — rather than one recorded request.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, Emulator, Reply, ACCOUNT_ID, REGION};

/// `TagResource`: `POST /tags/{resourceArn}` with the tag map in the body.
///
/// `resourceArn` is bound to a NON-greedy `@httpLabel`
/// (`smithy_query(query="operation",operation="TagResource")` reports
/// `POST /tags/{resourceArn}`), so the ARN travels as ONE path segment with its `:`
/// and `/` percent-encoded, exactly as an AWS SDK client sends it.
async fn tag_resource(emulator: &Emulator, resource_arn: &str, tags: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/tags/{}", encode_label(resource_arn)),
            Some(&json!({ "tags": tags })),
        )
        .await
}

/// `ListTagsForResource`: `GET /tags/{resourceArn}`.
async fn list_tags(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/tags/{}", encode_label(resource_arn)), None)
        .await
}

/// `value` as one path segment: everything outside the alphanumerics is
/// percent-encoded, so an ARN's `:` and `/` cannot be read as segment boundaries.
fn encode_label(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// A resource kind tagging addresses: the kinds the model gives a `TagMap` member
/// (`App$tags`, `Branch$tags`) and whose create accepts `tags`.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// An app, addressed by `…:apps/<appId>`.
    App,
    /// A branch under this name, addressed by `…:apps/<appId>/branches/<branchName>`.
    Branch(&'static str),
}

/// Both taggable kinds, the branch under an ordinary name and under one carrying a
/// slash — `BranchName` is modelled `length 1..255, pattern ^(?s).+$`
/// (`smithy_query(query="shape",shape="BranchName")`), so `release/1.0` is a name, and
/// its ARN's last segment therefore carries a slash of its own.
const KINDS: [Kind; 3] = [Kind::App, Kind::Branch("main"), Kind::Branch("release/1.0")];

/// A seeded resource: the ARN tagging addresses it by, and the ids its own reader takes.
struct Seeded {
    kind: Kind,
    app_id: String,
    arn: String,
}

/// Create one resource of `kind`, carrying `tags` from birth when the case sets them,
/// and answer what the suite needs to address it.
async fn seed(emulator: &Emulator, kind: Kind, tags: Option<&Value>) -> Seeded {
    match kind {
        Kind::App => {
            let mut request = json!({ "name": "tagging-host" });
            if let Some(tags) = tags {
                request["tags"] = tags.clone();
            }
            let reply = emulator.create_app(&request).await;
            assert_eq!(reply.status, 200, "CreateApp must succeed, got {reply:?}");
            let app = reply.member("app");
            Seeded {
                kind,
                app_id: string_member(&app, "appId"),
                arn: string_member(&app, "appArn"),
            }
        }
        Kind::Branch(branch_name) => {
            let app_id = emulator.app_id_of_new_app("tagging-host").await;
            let mut request = json!({ "branchName": branch_name });
            if let Some(tags) = tags {
                request["tags"] = tags.clone();
            }
            let reply = emulator.create_branch(&app_id, &request).await;
            assert_eq!(reply.status, 200, "CreateBranch must succeed, got {reply:?}");
            let branch = reply.member("branch");
            Seeded {
                kind,
                app_id,
                arn: string_member(&branch, "branchArn"),
            }
        }
    }
}

/// The `tags` member of the resource's OWN record, read back through `GetApp` /
/// `GetBranch`: the tag map has one home, so the record's reader reports what a tag
/// write left.
async fn record_tags(emulator: &Emulator, seeded: &Seeded) -> Value {
    let reply = match seeded.kind {
        Kind::App => emulator.get_app(&seeded.app_id).await,
        Kind::Branch(branch_name) => {
            emulator
                .call(
                    "GET",
                    &format!(
                        "/apps/{}/branches/{}",
                        seeded.app_id,
                        encode_label(branch_name)
                    ),
                    None,
                )
                .await
        }
    };
    assert_eq!(
        reply.status, 200,
        "the resource's own reader must answer 200, got {reply:?}"
    );
    let record = match seeded.kind {
        Kind::App => reply.member("app"),
        Kind::Branch(_) => reply.member("branch"),
    };
    record
        .get("tags")
        .cloned()
        .unwrap_or_else(|| panic!("the record reports no `tags` member: {record:?}"))
}

/// One tagging case: what the resource was created with, the tag writes applied to it
/// in order, and the map it must carry afterwards.
struct TagCase {
    case: &'static str,
    created_with: Option<Value>,
    writes: Vec<Value>,
    expected: Value,
}

/// The merge table.
///
/// It varies where the resource's existing tags came from (the create, an earlier write,
/// nowhere), whether the written keys collide with them, the number of writes, the
/// number of keys per write, and the key and value at the maxima `TagMap` models
/// (`TagKey length 1..128`, `TagValue length 0..256`, from
/// `smithy_query(query="shape",shape="TagMap")`).
fn tag_cases() -> Vec<TagCase> {
    // 128 characters, `TagKey`'s modelled maximum, over its pattern's alphabet.
    let longest_key = "k".repeat(128);
    // 256 characters, `TagValue`'s modelled maximum.
    let longest_value = "v".repeat(256);
    // 12 keys in one request: more than one, fewer than the modelled limit.
    let many: Value = Value::Object(
        (0..12)
            .map(|index| (format!("key.{index}"), json!(format!("value-{index}"))))
            .collect(),
    );

    vec![
        TagCase {
            case: "the first tags on a resource created without any",
            created_with: None,
            writes: vec![json!({ "env": "dev" })],
            expected: json!({ "env": "dev" }),
        },
        TagCase {
            case: "a disjoint key merges with the tags the create set",
            created_with: Some(json!({ "env": "dev" })),
            writes: vec![json!({ "owner": "platform" })],
            expected: json!({ "env": "dev", "owner": "platform" }),
        },
        TagCase {
            case: "a key the resource already carries takes the new value, others keep theirs",
            created_with: Some(json!({ "env": "dev", "owner": "platform" })),
            writes: vec![json!({ "env": "prod" })],
            expected: json!({ "env": "prod", "owner": "platform" }),
        },
        TagCase {
            case: "two writes of different keys converge on the union of both",
            created_with: None,
            writes: vec![json!({ "cost.center": "4711" }), json!({ "team/name": "web" })],
            expected: json!({ "cost.center": "4711", "team/name": "web" }),
        },
        TagCase {
            case: "a key written twice ends at the value the later write gave it",
            created_with: Some(json!({ "stage": "one" })),
            writes: vec![json!({ "stage": "two" }), json!({ "stage": "three" })],
            expected: json!({ "stage": "three" }),
        },
        TagCase {
            case: "the empty value is a value, not an absent tag",
            created_with: None,
            writes: vec![json!({ "env": "" })],
            expected: json!({ "env": "" }),
        },
        TagCase {
            case: "an empty tag map leaves the resource's tags exactly as they were",
            created_with: Some(json!({ "env": "dev" })),
            writes: vec![json!({})],
            expected: json!({ "env": "dev" }),
        },
        TagCase {
            case: "twelve keys in one write",
            created_with: None,
            writes: vec![many.clone()],
            expected: many,
        },
        TagCase {
            case: "a key and a value at their modelled length maxima",
            created_with: None,
            writes: vec![json!({ longest_key.clone(): longest_value.clone() })],
            expected: json!({ longest_key: longest_value }),
        },
        TagCase {
            case: "a value over the whole punctuation and letter range TagValue admits",
            created_with: Some(json!({ "keep": "me" })),
            writes: vec![json!({ "owner:group": "Zürich plus/minus=1 _.:@-" })],
            expected: json!({ "keep": "me", "owner:group": "Zürich plus/minus=1 _.:@-" }),
        },
    ]
}

#[tokio::test]
async fn a_tag_write_merges_into_the_tag_map_the_resource_carries() {
    for case in tag_cases() {
        for kind in KINDS {
            let emulator = Emulator::new();
            let seeded = seed(&emulator, kind, case.created_with.as_ref()).await;

            for write in &case.writes {
                let written = tag_resource(&emulator, &seeded.arn, write).await;
                assert_eq!(
                    written.status, 200,
                    "case {:?} ({kind:?}): TagResource {write} expected 200, got {written:?}",
                    case.case
                );
                assert!(
                    written.body == Value::Null || written.body == json!({}),
                    "case {:?} ({kind:?}): TagResource answers an empty body, got {:?}",
                    case.case,
                    written.body
                );
            }

            let listed = list_tags(&emulator, &seeded.arn).await;
            assert_eq!(
                listed.status, 200,
                "case {:?} ({kind:?}): ListTagsForResource expected 200, got {listed:?}",
                case.case
            );
            assert_eq!(
                listed.member("tags"),
                case.expected,
                "case {:?} ({kind:?}): expected the merged map {}, got {:?}",
                case.case,
                case.expected,
                listed.member("tags")
            );

            // The map has ONE home: the resource's own record reports the same thing.
            let on_the_record = record_tags(&emulator, &seeded).await;
            assert_eq!(
                on_the_record, case.expected,
                "case {:?} ({kind:?}): the resource record reports {on_the_record:?}, not the \
                 merged map {}",
                case.case, case.expected
            );
        }
    }
}

#[tokio::test]
async fn a_resource_with_no_tags_reports_the_empty_map_and_reading_changes_nothing() {
    for kind in KINDS {
        let emulator = Emulator::new();
        let seeded = seed(&emulator, kind, None).await;

        let first = list_tags(&emulator, &seeded.arn).await;
        assert_eq!(
            first.status, 200,
            "case {kind:?}: ListTagsForResource expected 200, got {first:?}"
        );
        assert_eq!(
            first.member("tags"),
            json!({}),
            "case {kind:?}: a resource with no tags reports the empty map, got {:?}",
            first.member("tags")
        );

        // A read is a report: the same question asked twice gets the same answer, and a
        // write after it merges into the empty map rather than into something a read
        // left behind.
        let again = list_tags(&emulator, &seeded.arn).await;
        assert_eq!(
            again.member("tags"),
            first.member("tags"),
            "case {kind:?}: a second read differs from the first: {:?} then {:?}",
            first.member("tags"),
            again.member("tags")
        );

        let written = tag_resource(&emulator, &seeded.arn, &json!({ "env": "dev" })).await;
        assert_eq!(
            written.status, 200,
            "case {kind:?}: TagResource after two reads expected 200, got {written:?}"
        );
        assert_eq!(
            list_tags(&emulator, &seeded.arn).await.member("tags"),
            json!({ "env": "dev" }),
            "case {kind:?}: the write after the reads must be the resource's whole tag map"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_taggable_resource_is_refused_and_leaves_the_store_alone() {
    let emulator = Emulator::new();

    // A tagged app and a webhook on it: the webhook exists, and `Webhook` models no tag
    // map (`smithy_query(query="shape",shape="Webhook")`), so its ARN names no resource
    // tagging can address.
    let seeded = seed(&emulator, Kind::App, Some(&json!({ "env": "dev" }))).await;
    let webhook = emulator
        .create_webhook(
            &seeded.app_id,
            &json!({ "branchName": "main", "description": "tagging" }),
        )
        .await;
    assert_eq!(webhook.status, 200, "CreateWebhook must succeed, got {webhook:?}");
    let webhook_arn = string_member(&webhook.member("webhook"), "webhookArn");

    // An `appId` no app in this store holds: it satisfies `AppId`'s modelled
    // `pattern ^d[a-z0-9]+$` (`smithy_query(query="shape",shape="AppId")`) and names
    // nothing.
    let absent_app_arn = format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/dabsent00000");

    for (label, arn, message) in [
        (
            "an app the store does not hold",
            absent_app_arn,
            "App dabsent00000 not found.".to_string(),
        ),
        (
            "a branch the addressed app does not hold",
            format!("{}/branches/nothing-here", seeded.arn),
            "Branch nothing-here not found.".to_string(),
        ),
        (
            "a webhook, a resource kind this service models no tag map on",
            webhook_arn.clone(),
            format!("Resource {webhook_arn} not found."),
        ),
        (
            "the apps collection with no app id after it",
            format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/"),
            format!("Resource arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/ not found."),
        ),
        (
            "an ARN of this service with an empty resource part",
            format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:"),
            format!("Resource arn:aws:amplify:{REGION}:{ACCOUNT_ID}: not found."),
        ),
    ] {
        for (operation, refused) in [
            ("TagResource", tag_resource(&emulator, &arn, &json!({ "env": "prod" })).await),
            ("ListTagsForResource", list_tags(&emulator, &arn).await),
        ] {
            assert_eq!(
                refused.status, 404,
                "case {label:?}: {operation} expected HTTP 404, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label:?}: {operation} expected ResourceNotFoundException, got {:?}",
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                Some(message.clone()),
                "case {label:?}: {operation} expected the message {message:?}, got {:?}",
                refused.message()
            );
            // `ResourceNotFoundException$code` is @required
            // (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
            assert_eq!(
                refused.body.get("code").and_then(Value::as_str),
                Some("ResourceNotFoundException"),
                "case {label:?}: {operation} must carry the error's code, got {:?}",
                refused.body
            );
        }
    }

    // None of the refusals reached the resource that DOES exist.
    assert_eq!(
        list_tags(&emulator, &seeded.arn).await.member("tags"),
        json!({ "env": "dev" }),
        "the refused writes must leave the tagged app exactly as it was"
    );
}

#[tokio::test]
async fn a_write_past_the_modelled_tag_limit_is_refused_and_stores_nothing() {
    // `TagMap` is modelled `length 0..50`
    // (`smithy_query(query="shape",shape="TagMap")`), and the map the resource carries
    // is a `TagMap`, so the bound is on what a write LEAVES.
    const LIMIT: usize = 50;

    for kind in KINDS {
        let emulator = Emulator::new();
        let seeded = seed(&emulator, kind, None).await;

        let at_the_limit: Value = Value::Object(
            (0..LIMIT)
                .map(|index| (format!("key.{index}"), json!(format!("value-{index}"))))
                .collect(),
        );
        let filled = tag_resource(&emulator, &seeded.arn, &at_the_limit).await;
        assert_eq!(
            filled.status, 200,
            "case {kind:?}: a write of exactly {LIMIT} tags expected 200, got {filled:?}"
        );

        // A key the resource already carries does not grow the map, so it is accepted
        // even with the map full.
        let overwrite = tag_resource(&emulator, &seeded.arn, &json!({ "key.0": "replaced" })).await;
        assert_eq!(
            overwrite.status, 200,
            "case {kind:?}: re-tagging a key the resource holds expected 200, got {overwrite:?}"
        );

        let refused = tag_resource(&emulator, &seeded.arn, &json!({ "one.too.many": "x" })).await;
        assert_eq!(
            refused.status, 400,
            "case {kind:?}: the {}th tag expected HTTP 400, got {refused:?}",
            LIMIT + 1
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {kind:?}: expected BadRequestException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            Some(format!(
                "Failed to tag resource {}. A resource can have a maximum of {LIMIT} tags, and \
                 these tags would leave it with {}.",
                seeded.arn,
                LIMIT + 1
            )),
            "case {kind:?}: unexpected refusal message {:?}",
            refused.message()
        );

        let listed = list_tags(&emulator, &seeded.arn).await;
        let tags = listed.member("tags");
        let stored = tags
            .as_object()
            .unwrap_or_else(|| panic!("case {kind:?}: `tags` is not a map in {listed:?}"));
        assert_eq!(
            stored.len(),
            LIMIT,
            "case {kind:?}: the refused write must store nothing, got {stored:?}"
        );
        assert!(
            !stored.contains_key("one.too.many"),
            "case {kind:?}: the refused key reached the store: {stored:?}"
        );
        assert_eq!(
            stored.get("key.0").and_then(Value::as_str),
            Some("replaced"),
            "case {kind:?}: the accepted overwrite is missing: {stored:?}"
        );
    }
}
