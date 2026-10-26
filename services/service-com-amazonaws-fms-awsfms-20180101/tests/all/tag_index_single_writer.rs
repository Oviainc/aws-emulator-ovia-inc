// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE rule the ARN-keyed tag index follows, driven
//! through every operation that maintains it.
//!
//! The rule, in one sentence: the entry the store holds under a resource's ARN
//! is exactly the tag set that resource's own record carries, and there is no
//! entry at all once no stored resource carries that ARN — so the index and the
//! authoritative record can never disagree, whichever operation last wrote or
//! removed the resource.
//!
//! Every taggable kind is created and updated through its own writer
//! (`PutPolicy`, `PutResourceSet`, `PutProtocolsList`, `PutAppsList` — the four
//! operations whose input carries a `TagList`) and removed through its own
//! deleter (`DeletePolicy`, `DeleteResourceSet`, `DeleteProtocolsList`,
//! `DeleteAppsList`), so each writer and each deleter is exercised here. The
//! index is read both off the wire (`ListTagsForResource`) and out of the store,
//! because an index that disagreed with the record would still answer the wire
//! correctly — the reader goes to the record — and only the store shows it.
//!
//! The table in [`rows`] is the class the rule ranges over: the rows differ in
//! how many tags the create sends (none at all, an empty list, one, several), in
//! whether the update GROWS, SHRINKS, empties or leaves the set alone (an absent
//! `TagList` on an update is "unchanged", so the index must keep the set it had),
//! and in key and value FORM up to the published `TagKey` / `TagValue` length
//! bounds (`com.amazonaws.fms#Tag`, from `smithy_query(shape="Tag")`: `TagKey`
//! 1..128, `TagValue` 0..256). Every assertion is made on every (writer, row)
//! pair.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_fms_awsfms_20180101::ops::types::Tag;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine};
// The taggable kinds (and each kind's ARN and `Delete*` request) and the tagging
// harness live in their own suites, so no two suites can drift in what they
// consider the same set of kinds or the same tag set.
use crate::tagging_across_resource_kinds::{Kind, KINDS};
use crate::tagging_and_managing_admins::{list_tags, minimal_policy, owned, tag_list};

/// One operation that writes a resource carrying a `TagList`: the kind it
/// writes, the member its resource document travels in (in the request and in
/// the response), the document members naming the resource's id and current
/// update token, and the output member carrying the ARN.
///
/// The member names are the modelled ones
/// (`smithy_query(operation="PutPolicy,PutResourceSet,PutProtocolsList,PutAppsList")`).
struct Writer {
    kind: Kind,
    operation: &'static str,
    document: &'static str,
    id: &'static str,
    token: &'static str,
    arn: &'static str,
}

/// Every writer whose input carries a `TagList`, one per taggable kind.
fn writers() -> Vec<Writer> {
    let writers = vec![
        Writer {
            kind: Kind::Policy,
            operation: "PutPolicy",
            document: "Policy",
            id: "PolicyId",
            token: "PolicyUpdateToken",
            arn: "PolicyArn",
        },
        Writer {
            kind: Kind::ResourceSet,
            operation: "PutResourceSet",
            document: "ResourceSet",
            id: "Id",
            token: "UpdateToken",
            arn: "ResourceSetArn",
        },
        Writer {
            kind: Kind::ProtocolsList,
            operation: "PutProtocolsList",
            document: "ProtocolsList",
            id: "ListId",
            token: "ListUpdateToken",
            arn: "ProtocolsListArn",
        },
        Writer {
            kind: Kind::AppsList,
            operation: "PutAppsList",
            document: "AppsList",
            id: "ListId",
            token: "ListUpdateToken",
            arn: "AppsListArn",
        },
    ];
    // Every taggable kind has a writer here: a fifth kind, or one left out,
    // is a gap in what these tests range over rather than a passing suite.
    for kind in KINDS {
        assert!(
            writers.iter().any(|writer| writer.kind == kind),
            "case {kind:?}: every taggable kind needs the writer that carries its TagList"
        );
    }
    writers
}

/// What a writer's call reported: the resource's ARN, its id and its current
/// update token.
struct Written {
    arn: String,
    id: String,
    token: String,
}

impl Writer {
    /// A minimal document of this writer's kind: every required member of the
    /// modelled shape and nothing else, because what these tests vary is the
    /// `TagList`, not the document.
    fn document_of(&self, name: &str) -> Value {
        match self.kind {
            Kind::Policy => minimal_policy(name),
            Kind::ResourceSet => json!({ "Name": name, "ResourceTypeList": ["AWS::EC2::VPC"] }),
            Kind::ProtocolsList => json!({ "ListName": name, "ProtocolsList": ["TCP", "UDP"] }),
            Kind::AppsList => json!({
                "ListName": name,
                "AppsList": [{ "AppName": "https", "Protocol": "tcp", "Port": 443 }],
            }),
        }
    }

    /// This writer's request: the document, the `TagList` (omitted entirely when
    /// `tags` is `None`) and, for an update, the resource's id and current token.
    fn request(&self, name: &str, tags: Option<&Value>, update: Option<&Written>) -> Value {
        let mut document = self.document_of(name);
        if let Some(written) = update {
            document[self.id] = json!(written.id);
            document[self.token] = json!(written.token);
        }
        let mut body = json!({});
        body[self.document] = document;
        if let Some(tags) = tags {
            body["TagList"] = tags.clone();
        }
        body
    }

    /// Call this writer, assert it answered 200, and hand back the identifiers
    /// and ARN it reported.
    async fn put(
        &self,
        router: &MultiServiceRouter,
        name: &str,
        tags: Option<&Value>,
        update: Option<&Written>,
        case: &str,
    ) -> Written {
        let (status, body) = call(router, self.operation, &self.request(name, tags, update)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {} to answer 200, got {status} with {body:?}",
            self.operation
        );
        let read = |member: &str| {
            body[self.document][member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {} reported no {member}: {body:?}",
                        self.operation
                    )
                })
                .to_string()
        };
        Written {
            arn: body[self.arn]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {} reported no {}: {body:?}",
                        self.operation, self.arn
                    )
                })
                .to_string(),
            id: read(self.id),
            token: read(self.token),
        }
    }
}

/// `tags` as the sorted key:value pairs the assertions compare against: tags are
/// a key:value MAP, so the comparison is order-insensitive.
fn pairs(tags: &[Tag]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect();
    pairs.sort();
    pairs
}

/// What the store holds for `arn`: the AUTHORITATIVE copy (the tags on the
/// resource's own record, resolved the way a tag reader resolves them) and the
/// ARN-keyed INDEX entry, each `None` where the store holds nothing.
type Held = (Option<Vec<(String, String)>>, Option<Vec<(String, String)>>);

fn held(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Held {
    with_store(engine, |store| {
        (
            store.tags_for_arn(arn).map(|tags| pairs(&tags)),
            store.tags_by_arn.get(arn).map(|tags| pairs(tags)),
        )
    })
}

/// Assert that the index entry for `arn` equals the record's tags and that both
/// equal `expected` — the whole rule, in the one place every case checks it.
fn assert_index_mirrors_record(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    expected: &[(String, String)],
    case: &str,
) {
    let (recorded, indexed) = held(engine, arn);
    assert_eq!(
        recorded.as_deref(),
        Some(expected),
        "case {case:?}: the resource's own record must carry {expected:?}, got {recorded:?}"
    );
    assert_eq!(
        indexed, recorded,
        "case {case:?}: the ARN-keyed index entry for {arn:?} must equal the record's tags \
         {recorded:?}, got {indexed:?}"
    );
}

/// One row of the tag table: what the create sends, what the update sends, and
/// the set the resource must hold after each.
struct Row {
    name: &'static str,
    /// The `TagList` the create sends, or `None` to send none at all.
    created: Option<Value>,
    /// The `TagList` the update sends, or `None` to send none at all — which is
    /// "unchanged", not "empty".
    updated: Option<Value>,
    after_create: Vec<(String, String)>,
    after_update: Vec<(String, String)>,
}

/// The tag-set class the rule is asserted over, on EVERY writer and deleter.
fn rows() -> Vec<Row> {
    // The published maxima of TagKey and TagValue, exactly.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        Row {
            name: "no TagList at all, then one tag",
            created: None,
            updated: Some(tag_list(&[("owner", "sec")])),
            after_create: Vec::new(),
            after_update: owned(&[("owner", "sec")]),
        },
        Row {
            name: "an empty TagList, then several tags",
            created: Some(tag_list(&[])),
            updated: Some(tag_list(&[("stage", "test"), ("team", "net"), ("owner", "ops")])),
            after_create: Vec::new(),
            after_update: owned(&[("stage", "test"), ("team", "net"), ("owner", "ops")]),
        },
        Row {
            // The SHRINK: the keys the update drops must leave the index too.
            name: "several tags, then a smaller set with one key rewritten",
            created: Some(tag_list(&[("stage", "test"), ("team", "net"), ("owner", "ops")])),
            updated: Some(tag_list(&[("stage", "prod")])),
            after_create: owned(&[("stage", "test"), ("team", "net"), ("owner", "ops")]),
            after_update: owned(&[("stage", "prod")]),
        },
        Row {
            name: "several tags, then an empty TagList",
            created: Some(tag_list(&[("a", "1"), ("b", "2")])),
            updated: Some(tag_list(&[])),
            after_create: owned(&[("a", "1"), ("b", "2")]),
            after_update: Vec::new(),
        },
        Row {
            // An absent TagList on an update leaves the set alone, so the index
            // has to still hold it afterwards.
            name: "one tag, then an update carrying no TagList at all",
            created: Some(tag_list(&[("only", "one")])),
            updated: None,
            after_create: owned(&[("only", "one")]),
            after_update: owned(&[("only", "one")]),
        },
        Row {
            name: "the longest modelled key and value, then another key",
            created: Some(Value::Array(vec![
                json!({ "Key": "k".repeat(128), "Value": "v".repeat(256) }),
            ])),
            updated: Some(tag_list(&[("dash-key", "")])),
            after_create: owned(&[(longest_key.as_str(), longest_value.as_str())]),
            after_update: owned(&[("dash-key", "")]),
        },
    ]
}

/// Delete the resource of `kind` that `id` names through that kind's own
/// deleter, asserting the delete answered 200.
async fn delete_resource(router: &MultiServiceRouter, kind: Kind, id: &str, case: &str) {
    let (operation, body) = kind.delete_request(id);
    let (status, answered) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation}({id:?}) to answer 200, got {status} with {answered:?}"
    );
}

/// Every writer and every deleter leaves the ARN-keyed index equal to the
/// resource's own record: the create's tag set, the update's tag set, and no
/// entry at all once the resource is deleted.
#[tokio::test]
async fn every_writer_and_deleter_keeps_the_index_equal_to_the_record() {
    for writer in writers() {
        for row in rows() {
            let case = format!("{} / {}", writer.operation, row.name);
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;

            // CREATE: the index holds what the create stored.
            let created = writer
                .put(&router, "indexed.resource", row.created.as_ref(), None, &case)
                .await;
            assert_eq!(
                created.arn,
                writer.kind.arn_of(&created.id),
                "case {case:?}: expected the ARN of the minted id {:?}, got {:?}",
                created.id,
                created.arn
            );
            let (status, code, reported) = list_tags(&router, &created.arn).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource({:?}) to answer 200, got {status} / \
                 {code:?}",
                created.arn
            );
            assert_eq!(
                reported, row.after_create,
                "case {case:?}: expected the created resource to report {:?}, got {reported:?}",
                row.after_create
            );
            assert_index_mirrors_record(&engine, &created.arn, &row.after_create, &case);

            // UPDATE through the same writer: same resource, same ARN, and the
            // index refreshed from the record the update wrote.
            let updated = writer
                .put(
                    &router,
                    "indexed.resource",
                    row.updated.as_ref(),
                    Some(&created),
                    &case,
                )
                .await;
            assert_eq!(
                (updated.arn.as_str(), updated.id.as_str()),
                (created.arn.as_str(), created.id.as_str()),
                "case {case:?}: an update must keep the resource's id and ARN, got {:?} / {:?}",
                updated.id,
                updated.arn
            );
            let (status, code, reported) = list_tags(&router, &created.arn).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource to answer 200 after the update, got \
                 {status} / {code:?}"
            );
            assert_eq!(
                reported, row.after_update,
                "case {case:?}: expected the updated resource to report {:?}, got {reported:?}",
                row.after_update
            );
            assert_index_mirrors_record(&engine, &created.arn, &row.after_update, &case);

            // DELETE through this kind's own deleter: no record, so no entry —
            // the tags do not outlive the resource.
            delete_resource(&router, writer.kind, &created.id, &case).await;
            let (recorded, indexed) = held(&engine, &created.arn);
            assert_eq!(
                recorded, None,
                "case {case:?}: the deleted resource must hold no record, got {recorded:?}"
            );
            assert_eq!(
                indexed, None,
                "case {case:?}: the deleted resource's ARN must carry no index entry, got \
                 {indexed:?}"
            );
            let (status, code, _) = list_tags(&router, &created.arn).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected ListTagsForResource({:?}) to answer 400 \
                 ResourceNotFoundException after the delete, got {status} / {code:?}",
                created.arn
            );
        }
    }
}

/// The index is maintained FROM the record rather than accumulated: an entry
/// that does not match the record is overwritten by the next write of that
/// resource and dropped by its delete, whatever it held before.
#[tokio::test]
async fn a_write_or_delete_rebuilds_the_entry_from_the_record() {
    let stale = vec![
        Tag {
            key: "stale".to_string(),
            value: "entry".to_string(),
        },
        Tag {
            key: "written".to_string(),
            value: "by-nobody".to_string(),
        },
    ];

    for writer in writers() {
        for delete_instead_of_update in [false, true] {
            let case = format!(
                "{} / {}",
                writer.operation,
                if delete_instead_of_update {
                    "then deleted"
                } else {
                    "then updated"
                }
            );
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;

            let held_tags = tag_list(&[("stage", "test"), ("team", "net")]);
            let created = writer
                .put(&router, "rebuilt.resource", Some(&held_tags), None, &case)
                .await;

            // Nothing a caller can do puts a disagreeing entry in the index, so
            // it is planted directly: what is asserted is that the next write or
            // delete of the resource repairs it rather than merging into it.
            with_store(&engine, |store| {
                store
                    .tags_by_arn
                    .insert(created.arn.clone(), stale.clone());
            });

            if delete_instead_of_update {
                delete_resource(&router, writer.kind, &created.id, &case).await;
                let (_, indexed) = held(&engine, &created.arn);
                assert_eq!(
                    indexed, None,
                    "case {case:?}: the delete must drop the entry whatever it held, got \
                     {indexed:?}"
                );
            } else {
                let rewritten = tag_list(&[("stage", "prod")]);
                writer
                    .put(
                        &router,
                        "rebuilt.resource",
                        Some(&rewritten),
                        Some(&created),
                        &case,
                    )
                    .await;
                assert_index_mirrors_record(
                    &engine,
                    &created.arn,
                    &owned(&[("stage", "prod")]),
                    &case,
                );
            }
        }
    }
}

/// One created resource: the kind it is, what its writer reported, and the tag
/// set it holds.
struct Resource {
    kind: Kind,
    written: Written,
    tags: Vec<(String, String)>,
}

/// One emulator holding a resource of every kind: each ARN's entry is its own
/// resource's tag set, and deleting one resource leaves every other entry in
/// place.
#[tokio::test]
async fn deleting_one_resource_leaves_the_other_kinds_entries_alone() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // One resource per kind, each with a tag set naming its kind.
    let mut resources: Vec<Resource> = Vec::new();
    for writer in writers() {
        let case = format!("{:?}", writer.kind);
        let tags = tag_list(&[("kind", case.as_str())]);
        let written = writer
            .put(&router, "one.per.kind", Some(&tags), None, &case)
            .await;
        resources.push(Resource {
            kind: writer.kind,
            written,
            tags: owned(&[("kind", case.as_str())]),
        });
    }

    for resource in &resources {
        assert_index_mirrors_record(
            &engine,
            &resource.written.arn,
            &resource.tags,
            &format!("{:?}", resource.kind),
        );
    }

    // Delete the resource set, the one kind whose deleter maintains the index
    // from its own handler rather than from the store module.
    let target = resources
        .iter()
        .position(|resource| resource.kind == Kind::ResourceSet)
        .expect("a resource of every taggable kind was created");
    let deleted = &resources[target];
    delete_resource(
        &router,
        deleted.kind,
        &deleted.written.id,
        &format!("{:?}", deleted.kind),
    )
    .await;

    for (index, resource) in resources.iter().enumerate() {
        let case = format!("{:?}", resource.kind);
        if index == target {
            let (recorded, indexed) = held(&engine, &resource.written.arn);
            assert_eq!(
                (recorded, indexed),
                (None, None),
                "case {case:?}: the deleted resource must leave neither record nor entry"
            );
        } else {
            assert_index_mirrors_record(&engine, &resource.written.arn, &resource.tags, &case);
        }
    }
}
