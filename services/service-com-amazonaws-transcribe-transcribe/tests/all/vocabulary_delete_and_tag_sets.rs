// SPDX-License-Identifier: CC0-1.0

//! Two rules of this service's vocabulary and tagging state, over the wire:
//!
//! 1. A delete in the custom-vocabulary family is IDEMPOTENT — a name this account
//!    holds no such resource under leaves nothing to do, so the call answers the
//!    operation's ordinary empty success and stores nothing. The only refusal left on
//!    that path is the documented non-terminal-state one (a `PENDING` vocabulary),
//!    which `crate::vocabularies` pins.
//! 2. A tag set this service REPORTS for a resource it holds is always present:
//!    `ListTagsForResource` answers an empty `Tags` list for a resource that carries
//!    no tags, whatever made it carry none, rather than leaving the member out.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for a created vocabulary's `PENDING -> READY` transition to have
/// fallen due (one `crate::state::VOCABULARY_TRANSITION_DELAY`, 500 ms, with room for
/// a slow machine), so a delete under test is answered for the resource's presence
/// rather than refused for its state.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Assert `reply` is a SUCCESS, whichever 2xx this protocol puts on the status line.
///
/// Which 2xx an empty awsJson1_1 success carries — the modelled `204` these
/// operations publish today or the `200` the recordings show — is the subject of its
/// own issue (`transcribe-awsjson1-1-success-responses-use-the-inert`). What the rules
/// here are about is success versus refusal, so they are asserted in a way that holds
/// for either.
pub(crate) fn succeeded(reply: &Reply, case: &str) {
    assert!(
        (200..300).contains(&reply.status),
        "case {case:?}: expected a 2xx success, got HTTP {} with body {:?}",
        reply.status,
        reply.body
    );
}

/// Assert `reply` is a client refusal, whichever 4xx shape this service renders it
/// with.
///
/// The status and `__type` of a missing-resource refusal are likewise another issue's
/// subject (`transcribe-client-errors-answer-the-modeled-httperror-instead`); what
/// matters here is that the read after a delete does NOT succeed.
fn refused(reply: &Reply, case: &str) {
    assert!(
        (400..500).contains(&reply.status),
        "case {case:?}: expected a 4xx refusal, got HTTP {} with body {:?}",
        reply.status,
        reply.body
    );
}

/// Assert a delete answered nothing but its empty success body: either no body at all
/// or the empty JSON object.
fn empty_body(reply: &Reply, case: &str) {
    let empty = match &reply.body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: a delete must answer an empty body, got {:?}",
        reply.body
    );
}

/// One member of the custom-vocabulary family: its delete, the member that names the
/// resource, the create that establishes one, and the read that proves presence.
struct Family {
    kind: &'static str,
    create: &'static str,
    delete: &'static str,
    read: &'static str,
    name_member: &'static str,
    /// This family's ARN resource type, as the `TranscribeArn` pattern's
    /// `<type>/<name>` segment spells it.
    resource_type: &'static str,
    /// The create members that carry this resource's content, as that operation
    /// models them (`Phrases` for a vocabulary, `Words` for a filter).
    content: fn(&str) -> Value,
}

/// The two resource families whose delete this suite spans. Both are created and
/// deleted by name within this service's own store, so no row depends on another
/// service or on an object store.
fn families() -> Vec<Family> {
    vec![
        Family {
            kind: "vocabulary",
            create: "CreateVocabulary",
            delete: "DeleteVocabulary",
            read: "GetVocabulary",
            name_member: "VocabularyName",
            resource_type: "vocabulary",
            content: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark", "Transcribe"]
                })
            },
        },
        Family {
            kind: "vocabulary filter",
            create: "CreateVocabularyFilter",
            delete: "DeleteVocabularyFilter",
            read: "GetVocabularyFilter",
            name_member: "VocabularyFilterName",
            resource_type: "vocabulary-filter",
            content: |name| {
                json!({
                    "VocabularyFilterName": name,
                    "LanguageCode": "en-US",
                    "Words": ["crud", "bogus"]
                })
            },
        },
    ]
}

/// The names a delete is probed with, one row per form the member's own
/// `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)` constraint allows
/// (`smithy_query(query="operation", operation="DeleteVocabulary")`): the shortest
/// name, a name using every allowed punctuation character, a long one, and one at the
/// published 200-character ceiling.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest allowed name", "a".to_string()),
        (
            "every allowed punctuation character",
            "a.b_c-d.0_9".to_string(),
        ),
        ("a plainly named resource", "never-created-at-all".to_string()),
        ("a name at the 200-character ceiling", "n".repeat(200)),
    ]
}

/// RULE 1: a delete of a custom-vocabulary-family resource this account does not hold
/// succeeds with an empty body, for every family and every form of name — and stores
/// nothing, so the listing stays empty.
#[tokio::test]
async fn deleting_a_resource_this_account_does_not_hold_succeeds() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (name_case, name) in absent_names() {
            let case = format!("{} / {name_case}", family.kind);
            let reply = transcribe
                .call(family.delete, json!({family.name_member: name}))
                .await;
            succeeded(&reply, &case);
            empty_body(&reply, &case);

            // The delete must not MINT what it did not find: the resource is still
            // absent to a reader afterwards.
            let read = transcribe
                .call(family.read, json!({family.name_member: name}))
                .await;
            refused(&read, &case);
        }
    }

    // Nor to a listing: both stores are still empty after every delete above.
    let vocabularies = transcribe
        .call("ListVocabularies", json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        vocabularies.ok("the vocabulary listing after absent deletes")["Vocabularies"],
        json!([]),
        "an absent delete must store nothing, got {:?}",
        vocabularies.body
    );
    let filters = transcribe
        .call("ListVocabularyFilters", json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        filters.ok("the filter listing after absent deletes")["VocabularyFilters"],
        json!([]),
        "an absent delete must store nothing, got {:?}",
        filters.body
    );
}

/// RULE 1, the repeat-delete axis: the delete that removed a resource answers the
/// same success when it is run again, as many times as a caller runs it.
#[tokio::test]
async fn a_delete_is_idempotent_once_the_resource_is_gone() {
    let transcribe = Transcribe::new();

    for family in families() {
        let name = format!("idempotent-{}", family.kind.replace(' ', "-"));
        let case = format!("{} / repeated delete", family.kind);
        succeeded(
            &transcribe
                .call(family.create, (family.content)(&name))
                .await,
            &case,
        );

        // A vocabulary is PENDING for the span its build is owed, and a delete while
        // it is still being processed is the documented refusal (pinned by
        // `crate::vocabularies`), so wait the span out before deleting.
        std::thread::sleep(PAST_THE_TRANSITION);

        for attempt in 1..=3 {
            let case = format!("{case} #{attempt}");
            let reply = transcribe
                .call(family.delete, json!({family.name_member: name}))
                .await;
            succeeded(&reply, &case);
            empty_body(&reply, &case);
            refused(
                &transcribe
                    .call(family.read, json!({family.name_member: name}))
                    .await,
                &case,
            );
        }
    }
}

/// RULE 1 does not reach across families: a delete answers for the resources of its
/// OWN type, so a vocabulary and a filter sharing one name are independent.
#[tokio::test]
async fn a_delete_only_removes_its_own_resource_family() {
    let transcribe = Transcribe::new();
    let shared = "shared.name";
    let case = "a vocabulary and a filter of the same name";

    succeeded(
        &transcribe
            .call("CreateVocabulary", (families()[0].content)(shared))
            .await,
        case,
    );
    succeeded(
        &transcribe
            .call("CreateVocabularyFilter", (families()[1].content)(shared))
            .await,
        case,
    );
    std::thread::sleep(PAST_THE_TRANSITION);

    let deleted = transcribe
        .call("DeleteVocabulary", json!({"VocabularyName": shared}))
        .await;
    succeeded(&deleted, case);
    refused(
        &transcribe
            .call("GetVocabulary", json!({"VocabularyName": shared}))
            .await,
        case,
    );
    // The filter of the same name is untouched, and the now-absent vocabulary delete
    // still succeeds.
    transcribe
        .call("GetVocabularyFilter", json!({"VocabularyFilterName": shared}))
        .await
        .ok(case);
    succeeded(
        &transcribe
            .call("DeleteVocabulary", json!({"VocabularyName": shared}))
            .await,
        case,
    );
    transcribe
        .call("GetVocabularyFilter", json!({"VocabularyFilterName": shared}))
        .await
        .ok(case);

    let removed = transcribe
        .call(
            "DeleteVocabularyFilter",
            json!({"VocabularyFilterName": shared}),
        )
        .await;
    succeeded(&removed, case);
    refused(
        &transcribe
            .call("GetVocabularyFilter", json!({"VocabularyFilterName": shared}))
            .await,
        case,
    );
}

/// One way a resource's tag set comes to be empty: the keys it is tagged with first,
/// and the `UntagResource` calls that then remove them.
struct Emptying {
    row: &'static str,
    keys: Vec<&'static str>,
    untags: Vec<Vec<&'static str>>,
}

/// The ways a tag set empties, one row per axis the rule generalises over: whether
/// the resource was ever tagged, how many keys are removed, in how many calls, and
/// whether the untag also names keys the resource does not carry.
fn emptyings() -> Vec<Emptying> {
    vec![
        Emptying {
            row: "never tagged",
            keys: vec![],
            untags: vec![],
        },
        Emptying {
            row: "one tag removed",
            keys: vec!["team"],
            untags: vec![vec!["team"]],
        },
        Emptying {
            row: "three tags removed in one call",
            keys: vec!["team", "env", "cost"],
            untags: vec![vec!["team", "env", "cost"]],
        },
        Emptying {
            row: "three tags removed one call at a time",
            keys: vec!["team", "env", "cost"],
            untags: vec![vec!["cost"], vec!["env"], vec!["team"]],
        },
        Emptying {
            row: "an untag naming keys the resource does not carry",
            keys: vec!["team"],
            untags: vec![vec!["team", "not-a-key", "another-absent-key"]],
        },
    ]
}

/// RULE 2: `ListTagsForResource` reports an EMPTY `Tags` list for a resource that
/// carries no tags, however it came to carry none — never by leaving the member out.
///
/// The rows differ along what the rule generalises over: the resource family, whether
/// the resource was ever tagged, how many keys were removed and in how many calls,
/// and whether the untag named keys the resource does not carry.
#[tokio::test]
async fn an_empty_tag_set_is_reported_as_an_empty_list() {
    let transcribe = Transcribe::new();

    let emptyings = emptyings();

    for family in families() {
        for (index, Emptying { row, keys, untags }) in emptyings.iter().enumerate() {
            let case = format!("{} / {row}", family.kind);
            let name = format!("tagless-{}-{index}", family.kind.replace(' ', "-"));
            let created = transcribe
                .call(family.create, (family.content)(&name))
                .await;
            succeeded(&created, &case);

            let resource_arn = format!(
                "arn:aws:transcribe:us-east-1:{account}:{resource_type}/{name}",
                account = engine::emulator_account_id(),
                resource_type = family.resource_type,
            );

            if !keys.is_empty() {
                let tags: Vec<Value> = keys
                    .iter()
                    .map(|key| json!({"Key": key, "Value": "any"}))
                    .collect();
                transcribe
                    .call(
                        "TagResource",
                        json!({"ResourceArn": resource_arn, "Tags": tags}),
                    )
                    .await
                    .ok(&case);
                let listed = transcribe
                    .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
                    .await;
                let reported = listed.ok(&case)["Tags"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {:?}", listed.body))
                    .len();
                assert_eq!(
                    reported,
                    keys.len(),
                    "case {case:?}: a tagged resource must report every tag, got {:?}",
                    listed.body
                );
            }

            for untag in untags {
                succeeded(
                    &transcribe
                        .call(
                            "UntagResource",
                            json!({"ResourceArn": resource_arn, "TagKeys": untag}),
                        )
                        .await,
                    &case,
                );
            }

            let empty = transcribe
                .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
                .await;
            empty.ok(&case);
            assert_eq!(
                empty.member(&case, "ResourceArn"),
                resource_arn,
                "case {case:?}: the answer must name the resource asked about, got {:?}",
                empty.body
            );
            assert_eq!(
                empty.body.get("Tags"),
                Some(&json!([])),
                "case {case:?}: a resource carrying no tags must report an empty Tags \
                 list, got {:?}",
                empty.body
            );

            // The empty list is the resource's CURRENT tag set, not a constant: a tag
            // added afterwards is reported, and removing it reports empty again.
            transcribe
                .call(
                    "TagResource",
                    json!({
                        "ResourceArn": resource_arn,
                        "Tags": [{"Key": "re-tagged", "Value": "yes"}]
                    }),
                )
                .await
                .ok(&case);
            let retagged = transcribe
                .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
                .await;
            assert_eq!(
                retagged.ok(&case)["Tags"],
                json!([{"Key": "re-tagged", "Value": "yes"}]),
                "case {case:?}: a tag added after the set emptied must be reported, got {:?}",
                retagged.body
            );
            succeeded(
                &transcribe
                    .call(
                        "UntagResource",
                        json!({"ResourceArn": resource_arn, "TagKeys": ["re-tagged"]}),
                    )
                    .await,
                &case,
            );
            let again = transcribe
                .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
                .await;
            assert_eq!(
                again.ok(&case)["Tags"],
                json!([]),
                "case {case:?}: removing the last tag again must report an empty list, \
                 got {:?}",
                again.body
            );
        }
    }
}
