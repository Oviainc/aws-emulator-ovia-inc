// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one rule EVERY Firewall Manager operation that
//! writes a `TagList` follows, asserted on the resource writers rather than on
//! the tagging operation.
//!
//! The rule, in one sentence: a `com.amazonaws.fms#Tag` is one key and one value
//! and a resource's tags are a key:value map, so a `TagList` a request carries
//! that names one tag key more than once describes no storable tag set and is
//! refused with the modelled `InvalidInputException` before anything is written
//! — whichever operation carries the `TagList`, and whether it creates or
//! updates the resource.
//!
//! `smithy_query` shows four writers carrying a `TagList` input beside
//! `TagResource` — `PutPolicy`, `PutResourceSet`, `PutProtocolsList` and
//! `PutAppsList`, each of which also models `InvalidInputException` — and
//! `Tag$Key`'s own documentation says "Tag keys are case-sensitive", so keys
//! differing only in case are two keys and not one repeated.
//!
//! The tables below are the class the rule ranges over: the rows are the
//! CARTESIAN product of the WRITER (so of the kind of resource written) and the
//! `TagList`, whose rows differ in how many times a key is repeated (twice,
//! three times), in whether the repeated entries carry the same value, in where
//! in the list the repeat falls (adjacent, among other keys, at the end of a
//! fifty-entry list) and in key length (up to the modelled `TagKey` maximum).
//! The accepted table varies the same dimensions where no key repeats (absent,
//! empty, distinct keys, keys differing only in case, one value under two keys,
//! fifty keys), and pins that such a set is stored, reported once per key and
//! still repairable through a later `TagResource`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, error_message, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};
// The taggable-resource kinds and the tagging harness (how a `TagList` is built
// and read back off the wire) live in their own suites, so no two suites can
// drift in what they consider the same tag set or the same set of kinds.
use crate::tagging_across_resource_kinds::Kind;
use crate::tagging_and_managing_admins::{
    list_tags, minimal_policy, owned, tag_list, tag_list_of_size, tag_pairs,
};

/// A well-formed ARN whose resource was never created — enough for the
/// `TagResource` cross-check below, because the repeated-key refusal is decided
/// from the `TagList` alone, before any resource is resolved.
const UNKNOWN_POLICY_ID: &str = "99999999-8888-7777-6666-555555555555";

/// One operation that writes a resource carrying a `TagList`: which operation it
/// is, the member its resource document travels in (in the request and in the
/// response), and the members of that document naming the resource's id and
/// current update token.
struct Writer {
    kind: Kind,
    operation: &'static str,
    document: &'static str,
    id: &'static str,
    token: &'static str,
    arn: &'static str,
}

/// Every writer whose input carries a `TagList`, from
/// `smithy_query(query="operation")`: `PutPolicy`, `PutResourceSet`,
/// `PutProtocolsList` and `PutAppsList`.
fn writers() -> Vec<Writer> {
    vec![
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
    ]
}

/// What one writer's create returned: the ARN the tagging operations name the
/// resource by, and the id and token a later update needs.
struct Created {
    arn: String,
    id: String,
    token: String,
}

impl Writer {
    /// A minimal document of this writer's resource kind: every required member
    /// of the modelled shape and nothing else, because what these tests vary is
    /// the `TagList`, not the document.
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
    fn request(&self, name: &str, tags: Option<&Value>, update: Option<(&str, &str)>) -> Value {
        let mut document = self.document_of(name);
        if let Some((id, token)) = update {
            document[self.id] = json!(id);
            document[self.token] = json!(token);
        }
        let mut body = json!({});
        body[self.document] = document;
        if let Some(tags) = tags {
            body["TagList"] = tags.clone();
        }
        body
    }

    /// Create one resource through this writer, assert it answered 200 and hand
    /// back the identifiers and ARN it reported.
    async fn create(
        &self,
        router: &MultiServiceRouter,
        name: &str,
        tags: Option<&Value>,
        case: &str,
    ) -> Created {
        let (status, body) = call(router, self.operation, &self.request(name, tags, None)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {} to answer 200, got {status} with {body:?}",
            self.operation
        );
        let document = &body[self.document];
        let read = |member: &str| {
            document[member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {} reported no {member}: {body:?}",
                        self.operation
                    )
                })
                .to_string()
        };
        let created = Created {
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
        };
        assert_eq!(
            created.arn,
            self.kind.arn_of(&created.id),
            "case {case:?}: expected the ARN of the minted id {:?}, got {:?}",
            created.id,
            created.arn
        );
        created
    }

    /// How many resources of this writer's kind the store holds.
    fn stored_count(&self, engine: &std::sync::Arc<engine::InMemoryEngine>) -> usize {
        with_store(engine, |store| match self.kind {
            Kind::Policy => store.policies.len(),
            Kind::ResourceSet => store.resource_sets.len(),
            Kind::ProtocolsList => store.protocols_lists.len(),
            Kind::AppsList => store.apps_lists.len(),
        })
    }
}

/// One `TagList` that names a tag key more than once: the row's name, the list
/// itself, and the key the refusal has to name.
struct Duplicate {
    name: &'static str,
    tags: Value,
    key: String,
}

/// The repeated-key class the refusal is asserted over, on EVERY writer.
///
/// The rows differ in how many times a key repeats, in whether the repeated
/// entries carry the same value, in where in the list the repeat falls and in
/// key length — the longest row's key is the published `TagKey` maximum
/// (`com.amazonaws.fms#Tag$Key`, length 1..128).
fn duplicates() -> Vec<Duplicate> {
    let longest_key = "k".repeat(128);
    // A fifty-entry list of distinct keys with one of its earliest keys repeated
    // at the very end, so the repeat is nowhere near its first occurrence.
    let mut long = tag_list_of_size(50);
    long.as_array_mut()
        .expect("tag_list_of_size builds an array")
        .push(json!({ "Key": "key0", "Value": "again" }));
    vec![
        Duplicate {
            name: "the same key twice",
            tags: tag_list(&[("k", "1"), ("k", "2")]),
            key: "k".to_string(),
        },
        Duplicate {
            name: "a repeated key among other keys",
            tags: tag_list(&[("a", "1"), ("b", "2"), ("a", "3")]),
            key: "a".to_string(),
        },
        Duplicate {
            // Repeating a key with the SAME value is still two entries for one
            // key, which a key:value map cannot hold.
            name: "a key repeated with the same value",
            tags: tag_list(&[("same", "v"), ("same", "v")]),
            key: "same".to_string(),
        },
        Duplicate {
            name: "one key three times",
            tags: tag_list(&[("k", "1"), ("k", "2"), ("k", "3")]),
            key: "k".to_string(),
        },
        Duplicate {
            name: "a key repeated at the end of a fifty-entry list",
            tags: long,
            key: "key0".to_string(),
        },
        Duplicate {
            name: "the longest modelled key repeated",
            tags: tag_list(&[(longest_key.as_str(), "1"), (longest_key.as_str(), "2")]),
            key: longest_key,
        },
    ]
}

/// One `TagList` that repeats no key, so the writer has to store it.
struct Accepted {
    name: &'static str,
    /// The `TagList` the request carries, or `None` to send none at all.
    tags: Option<Value>,
}

/// The accepted class, on EVERY writer: the rows differ in tag count (absent,
/// empty, two, fifty), in whether two keys differ only in CASE (`Tag$Key`: "Tag
/// keys are case-sensitive", so those are two keys and not one repeated) and in
/// whether one value appears under two different keys.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            name: "no TagList at all",
            tags: None,
        },
        Accepted {
            name: "an empty TagList",
            tags: Some(tag_list(&[])),
        },
        Accepted {
            name: "two distinct keys",
            tags: Some(tag_list(&[("a", "1"), ("b", "2")])),
        },
        Accepted {
            name: "keys differing only in case",
            tags: Some(tag_list(&[("env", "1"), ("Env", "2")])),
        },
        Accepted {
            name: "one value under two distinct keys",
            tags: Some(tag_list(&[("a", "v"), ("b", "v")])),
        },
        Accepted {
            name: "fifty distinct keys",
            tags: Some(tag_list_of_size(50)),
        },
    ]
}

/// The tag set `tags` describes, as the sorted key:value pairs a reader reports.
fn expected_set(tags: Option<&Value>) -> Vec<(String, String)> {
    tags.map(tag_pairs).unwrap_or_default()
}

/// The keys a reported tag set carries, in the order it reported them.
fn keys(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(key, _)| key.clone()).collect()
}

/// A CREATE whose `TagList` names one key more than once is refused with the
/// modelled `InvalidInputException`, names the repeated key, answers exactly
/// what the tagging operation answers for the same list, and creates nothing.
#[tokio::test]
async fn a_create_whose_tag_list_repeats_a_key_is_refused_and_stores_nothing() {
    for writer in writers() {
        for duplicate in duplicates() {
            let case = format!("{} / {}", writer.operation, duplicate.name);
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;

            let (status, body) = call(
                &router,
                writer.operation,
                &writer.request("duplicate.tags", Some(&duplicate.tags), None),
            )
            .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInputException"),
                "case {case:?}: expected 400 InvalidInputException, got {status} / {code:?} with \
                 {body:?}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&duplicate.key),
                "case {case:?}: the refusal must name the repeated key {:?}, got {message:?}",
                duplicate.key
            );

            // The same `TagList` through the tagging operation: one rule, one
            // refusal, so the writer may not word it differently.
            let (tag_status, tag_body) = call(
                &router,
                "TagResource",
                &json!({
                    "ResourceArn": Kind::Policy.arn_of(UNKNOWN_POLICY_ID),
                    "TagList": duplicate.tags,
                }),
            )
            .await;
            assert_eq!(
                (tag_status, error_code(&tag_body), error_message(&tag_body)),
                (status, code.clone(), message.clone()),
                "case {case:?}: the writer must answer what TagResource answers for the same \
                 TagList, got {tag_body:?} against {body:?}"
            );

            // A refused create persisted nothing: no resource of the kind, and
            // no tag entry under any ARN.
            let stored = writer.stored_count(&engine);
            assert_eq!(
                stored, 0,
                "case {case:?}: a refused create must store no resource, got {stored}"
            );
            let indexed = with_store(&engine, |store| store.tags_by_arn.len());
            assert_eq!(
                indexed, 0,
                "case {case:?}: a refused create must leave no tag state behind, got {indexed} \
                 indexed ARNs"
            );
        }
    }
}

/// An UPDATE whose `TagList` names one key more than once is refused the same
/// way and changes nothing: the resource keeps the tag set it held, and its
/// update token is not rotated — so the token the refused call carried still
/// drives a later update.
#[tokio::test]
async fn an_update_whose_tag_list_repeats_a_key_is_refused_and_changes_nothing() {
    let held = [("stage", "test"), ("team", "net")];
    let rewritten = [("stage", "prod")];

    for writer in writers() {
        for duplicate in duplicates() {
            let case = format!("{} / {}", writer.operation, duplicate.name);
            let (router, _engine) = emulator_with_engine();
            associate_admin(&router).await;
            let created = writer
                .create(&router, "updated.resource", Some(&tag_list(&held)), &case)
                .await;

            let (status, body) = call(
                &router,
                writer.operation,
                &writer.request(
                    "updated.resource",
                    Some(&duplicate.tags),
                    Some((&created.id, &created.token)),
                ),
            )
            .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInputException"),
                "case {case:?}: expected the update to answer 400 InvalidInputException, got \
                 {status} / {code:?} with {body:?}"
            );

            let (status, read_code, tags) = list_tags(&router, &created.arn).await;
            assert_eq!(
                (status, read_code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource to answer 200 after the refusal, got \
                 {status} / {read_code:?}"
            );
            assert_eq!(
                tags,
                owned(&held),
                "case {case:?}: a refused update must leave the tag set alone, got {tags:?}"
            );

            // Nothing was written, so the token the refused call carried is
            // still the current one and a well-formed update still lands.
            let (status, body) = call(
                &router,
                writer.operation,
                &writer.request(
                    "updated.resource",
                    Some(&tag_list(&rewritten)),
                    Some((&created.id, &created.token)),
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused update must not rotate the update token, but the \
                 following update answered {status} with {body:?}"
            );
            let (_, _, tags) = list_tags(&router, &created.arn).await;
            assert_eq!(
                tags,
                owned(&rewritten),
                "case {case:?}: expected the accepted update's TagList to replace the set, got \
                 {tags:?}"
            );
        }
    }
}

/// A `TagList` that repeats no key is stored by every writer, reported ONCE per
/// key, and stays repairable: a later `TagResource` for a key the resource
/// already carries replaces that key's value instead of adding a second entry
/// for it.
#[tokio::test]
async fn a_tag_list_of_distinct_keys_is_stored_once_per_key_and_stays_repairable() {
    for writer in writers() {
        for row in accepted() {
            let case = format!("{} / {}", writer.operation, row.name);
            let (router, _engine) = emulator_with_engine();
            associate_admin(&router).await;
            let created = writer
                .create(&router, "distinct.tags", row.tags.as_ref(), &case)
                .await;

            let expected = expected_set(row.tags.as_ref());
            let (status, code, tags) = list_tags(&router, &created.arn).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource to answer 200, got {status} / \
                 {code:?}"
            );
            assert_eq!(
                tags, expected,
                "case {case:?}: expected the created resource to hold {expected:?}, got {tags:?}"
            );
            let mut distinct = keys(&tags);
            distinct.dedup();
            assert_eq!(
                distinct.len(),
                tags.len(),
                "case {case:?}: every key must be reported once, got {:?}",
                keys(&tags)
            );

            // The repair: a write for a key the resource already carries (or a
            // new key, when it carries none) leaves exactly one entry for it.
            let target = tags
                .first()
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| "owner".to_string());
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({
                    "ResourceArn": created.arn,
                    "TagList": [{ "Key": target, "Value": "repaired" }],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected TagResource to answer 200, got {status} with {body:?}"
            );
            let (_, _, tags) = list_tags(&router, &created.arn).await;
            let holding: Vec<&(String, String)> =
                tags.iter().filter(|(key, _)| key == &target).collect();
            assert_eq!(
                holding,
                vec![&(target.clone(), "repaired".to_string())],
                "case {case:?}: the key {target:?} must carry exactly one, rewritten value, got \
                 {tags:?}"
            );
        }
    }
}
