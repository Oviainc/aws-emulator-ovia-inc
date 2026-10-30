// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SUCCESS STATUS of this endpoint's tagging mutations, driven
//! over HTTP through the same router the emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: an operation of this endpoint whose output structure declares NO members
//! answers a success with 204 No Content and no document — so `TagResource` and
//! `UntagResource`, whose `TagResourceResponse` / `UntagResourceResponse` carry no
//! members at all, answer 204, while an operation whose response DOES carry members keeps
//! the status that announces a document (`ListTagsForResource`, 200 with its `tags`
//! object). An operation whose model states an explicit `@http` code keeps the stated one
//! (`smithy_query(query="operation",operation="DeleteAssistant")`: `code: 204`), and a
//! REFUSAL still answers its own 4xx: the rule is about the success path only.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * the resource family the ARN belongs to — an assistant, a knowledge base, a session
//!   and an assistant association, the four families this model's implemented surface
//!   creates;
//! * what the resource already holds and what the call carries: no pair, one, several,
//!   keys that collide with stored ones, keys that do not, both published length bounds
//!   of `TagKey` and `TagValue`, and the punctuation the modelled `TagKey` pattern
//!   admits;
//! * for the removal: a key the resource holds, a key it does not, several at once, every
//!   key it holds, and both published bounds of the modelled `tagKeys` list (1 and 50)
//!   plus one past the maximum;
//! * the method the mutation arrives on (`POST` for the merge, `DELETE` for the removal),
//!   and whether the call repeats one that already happened (both are `@idempotent`).
//!
//! Every row also reads the result back over the wire through `ListTagsForResource`, so a
//! handler that answered the status without doing the work cannot pass. The harness (the
//! router, the creates, the ARN forms) is the one `resource_tags.rs` already drives this
//! surface with.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::resource_tags::{
    create_tagged, label, tag_map, wisdom_arn, Emulator, Family, Reply, ABSENT_UUID, FAMILIES,
    REGION, TAGGING_NOT_FOUND_CODE,
};

/// The status a member-less response of this endpoint is answered with. Written out here
/// rather than imported from the implementation, so the assertion pins the status line a
/// client sees.
const NO_CONTENT: u16 = 204;

/// The status a response that DOES carry a document is answered with.
const WITH_DOCUMENT: u16 = 200;

/// The published bounds of the modelled tag map's key and value
/// (`smithy_query(query="shape",shape="Tags")`: `TagKey` len 1..128, `TagValue` len
/// 1..256).
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;

/// The published bounds of the modelled removal list
/// (`smithy_query(query="operation",operation="UntagResource")`: `tagKeys` len 1..50).
const TAG_KEYS_MIN: usize = 1;
const TAG_KEYS_MAX: usize = 50;

/// Keys and values written as a map, so a row reads as the pairs it is about.
fn pairs(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `TagResource`: the merge arrives as a `POST` with the map in the body.
async fn tag_resource(
    emulator: &Emulator,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `UntagResource`: the removal arrives as a `DELETE` with the keys in the query string,
/// one `tagKeys` parameter per key, each percent-encoded as an SDK encodes it.
async fn untag_resource(emulator: &Emulator, resource_arn: &str, keys: &[String]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .send(
            "DELETE",
            &format!("/tags/{}?{query}", label(resource_arn)),
            None,
        )
        .await
}

/// One answer that must carry no content: the no-content status, no error code, and a
/// body with no members.
fn assert_no_content(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, NO_CONTENT,
        "case {case:?}: expected the no-content status {NO_CONTENT}, got {} with {}",
        reply.status, reply.body
    );
    assert!(
        reply.error_type.is_empty(),
        "case {case:?}: expected no modelled failure, got x-amzn-errortype {:?} with {}",
        reply.error_type,
        reply.body
    );
    let members = match &reply.body {
        Value::Null => 0,
        Value::Object(members) => members.len(),
        other => panic!("case {case:?}: expected no document in the body, got {other}"),
    };
    assert_eq!(
        members, 0,
        "case {case:?}: expected a body carrying no members, got {}",
        reply.body
    );
}

/// The tag map `ListTagsForResource` reports for one ARN, so a row compares what is
/// PERSISTED rather than what a mutation happened to answer.
async fn stored_tags(emulator: &Emulator, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let listed = emulator.list_tags(resource_arn).await;
    // The control for the rule: this response DOES carry members, so it keeps the status
    // that announces a document.
    assert_eq!(
        listed.status, WITH_DOCUMENT,
        "case {case:?}: expected ListTagsForResource to answer HTTP {WITH_DOCUMENT} for \
         {resource_arn}, got {} with {}",
        listed.status, listed.body
    );
    assert!(
        listed.field("/tags").is_some(),
        "case {case:?}: expected ListTagsForResource to carry a tags document, got {}",
        listed.body
    );
    serde_json::from_value(listed.field("/tags").cloned().unwrap_or(json!({})))
        .unwrap_or_else(|error| panic!("case {case:?}: expected a tag object, got {error}"))
}

/// One row: what the resource is created holding, what the merge carries, and which keys
/// the removal names.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    initial: BTreeMap<String, String>,
    merge: BTreeMap<String, String>,
    remove: Vec<String>,
}

fn keys_of(tags: &BTreeMap<String, String>) -> Vec<String> {
    tags.keys().cloned().collect()
}

/// The table: every shape of merge and of removal the rule spans.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one new key, then the one key removed",
            initial: BTreeMap::new(),
            merge: tag_map(TAG_KEYS_MIN),
            remove: keys_of(&tag_map(TAG_KEYS_MIN)),
        },
        Row {
            label: "several new keys, then two of them removed",
            initial: tag_map(2),
            merge: pairs(&[("fresh", "added"), ("other", "added")]),
            remove: vec!["fresh".to_string(), "other".to_string()],
        },
        Row {
            label: "a stored key re-valued, then a key the resource does not hold removed",
            initial: tag_map(2),
            merge: pairs(&[("key-0", "replaced")]),
            remove: vec!["never-stored".to_string()],
        },
        Row {
            label: "an empty merge, then every stored key removed",
            initial: tag_map(3),
            merge: BTreeMap::new(),
            remove: keys_of(&tag_map(3)),
        },
        Row {
            // Source: the modelled `TagKey` pattern, `^(?!aws:)[a-zA-Z+-=._:/]+$`
            // (`smithy_query(query="shape",shape="Tags")`) — punctuation the issue's
            // recording never carried.
            label: "the punctuation the modelled TagKey pattern admits",
            initial: pairs(&[("Env", "Staging")]),
            merge: pairs(&[
                ("Env", "Production"),
                ("a.b", "1"),
                ("path/sub", "nested"),
                ("plus+eq=", "signs"),
                ("ns:name", "colon"),
            ]),
            remove: vec!["path/sub".to_string(), "ns:name".to_string()],
        },
        Row {
            label: "the published length bounds of TagKey and TagValue",
            initial: tag_map(1),
            merge: [
                ("k".repeat(TAG_KEY_MAX), "v".repeat(TAG_VALUE_MAX)),
                ("k".to_string(), "v".to_string()),
            ]
            .into_iter()
            .collect(),
            remove: vec!["k".repeat(TAG_KEY_MAX)],
        },
        Row {
            label: "the published maximum number of keys in one removal",
            initial: tag_map(TAG_KEYS_MAX),
            merge: BTreeMap::new(),
            remove: keys_of(&tag_map(TAG_KEYS_MAX)),
        },
    ]
}

/// For every family and every row, both tagging mutations answer the no-content status
/// with no document — and the work they report is the work a later read sees.
#[tokio::test]
async fn the_tagging_mutations_answer_the_no_content_status_with_no_document() {
    let emulator = Emulator::new();
    let host = emulator
        .create_assistant("no-content-host-assistant", &BTreeMap::new())
        .await;
    assert_eq!(
        host.status, WITH_DOCUMENT,
        "case \"host assistant\": expected HTTP {WITH_DOCUMENT}, got {} with {}",
        host.status, host.body
    );
    let host_assistant = host.string("/assistant/assistantId");

    for (seq, row) in rows().into_iter().enumerate() {
        for family in FAMILIES {
            let seq = 7_000 + seq * FAMILIES.len() + family as usize;
            let (arn, _) = create_tagged(
                &emulator,
                family,
                seq,
                row.label,
                &host_assistant,
                &row.initial,
            )
            .await;

            let merge_case = format!("merge for {:?} ({family:?})", row.label);
            let tagged = tag_resource(&emulator, &arn, &row.merge).await;
            assert_no_content(&tagged, &merge_case);

            // The status is not a constant the handler answers without working: what the
            // merge reported is what a later read reports.
            let mut expected = row.initial.clone();
            for (key, value) in &row.merge {
                expected.insert(key.clone(), value.clone());
            }
            let merged = stored_tags(&emulator, &merge_case, &arn).await;
            assert_eq!(
                merged, expected,
                "case {merge_case:?}: expected {arn} to hold {expected:?}, got {merged:?}"
            );

            // `@idempotent`: the repeat answers the same status, with no document either.
            let repeated = tag_resource(&emulator, &arn, &row.merge).await;
            assert_no_content(&repeated, &format!("repeated {merge_case}"));

            let removal_case = format!("removal for {:?} ({family:?})", row.label);
            let untagged = untag_resource(&emulator, &arn, &row.remove).await;
            assert_no_content(&untagged, &removal_case);

            for key in &row.remove {
                expected.remove(key);
            }
            let remaining = stored_tags(&emulator, &removal_case, &arn).await;
            assert_eq!(
                remaining, expected,
                "case {removal_case:?}: expected {arn} to hold {expected:?} after removing \
                 {:?}, got {remaining:?}",
                row.remove
            );

            let repeated = untag_resource(&emulator, &arn, &row.remove).await;
            assert_no_content(&repeated, &format!("repeated {removal_case}"));
        }
    }
}

/// The rule covers the SUCCESS path only, and only a member-less response: a refused
/// mutation keeps its own 4xx answer with the error document a client branches on, a
/// removal naming more keys than the modelled list admits is refused rather than answered
/// with the no-content status, and the reporting operation of the same path keeps the
/// status that announces its document.
#[tokio::test]
async fn a_refusal_and_a_response_carrying_members_keep_their_own_status() {
    let emulator = Emulator::new();
    let host = emulator
        .create_assistant("no-content-control-assistant", &BTreeMap::new())
        .await;
    let host_assistant = host.string("/assistant/assistantId");
    let assistant_arn = host.string("/assistant/assistantArn");

    // An ARN of each family naming nothing this account holds: neither mutation may
    // answer the no-content status for it.
    for family in FAMILIES {
        let unknown = wisdom_arn(
            "aws",
            REGION,
            &emulator_account(&assistant_arn),
            &format!("{}/{ABSENT_UUID}", family.arn_label()),
        );
        for (operation, reply) in [
            (
                "TagResource",
                tag_resource(&emulator, &unknown, &tag_map(1)).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, &unknown, &["key-0".to_string()]).await,
            ),
        ] {
            let case = format!("{operation} on an ARN of {family:?} naming nothing");
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404 for {unknown}, got {} with {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.contains(TAGGING_NOT_FOUND_CODE),
                "case {case:?}: expected the tagging surface's {TAGGING_NOT_FOUND_CODE}, got \
                 x-amzn-errortype {:?} with {}",
                reply.error_type,
                reply.body
            );
        }
    }

    // One past the published maximum of the modelled removal list: a refusal, never the
    // no-content status.
    let (arn, _) = create_tagged(
        &emulator,
        Family::KnowledgeBase,
        7_900,
        "one past the removal maximum",
        &host_assistant,
        &tag_map(3),
    )
    .await;
    let too_many: Vec<String> = keys_of(&tag_map(TAG_KEYS_MAX + 1));
    let refused = untag_resource(&emulator, &arn, &too_many).await;
    let case = format!("a removal naming {} keys", too_many.len());
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected a request naming more than {TAG_KEYS_MAX} keys to be \
         refused, got {} with {}",
        refused.status, refused.body
    );
    assert!(
        refused.error_type.contains("BadRequestException"),
        "case {case:?}: expected the code this endpoint refuses a value outside a modelled \
         bound with, got x-amzn-errortype {:?} with {}",
        refused.error_type,
        refused.body
    );
    let still_there = stored_tags(&emulator, &case, &arn).await;
    assert_eq!(
        still_there,
        tag_map(3),
        "case {case:?}: expected a refused removal to store nothing, got {still_there:?}"
    );

    // A removal at the published minimum is served, and the reporting operation of the
    // same path keeps the status that announces its document.
    let at_minimum = untag_resource(&emulator, &arn, &keys_of(&tag_map(TAG_KEYS_MIN))).await;
    assert_no_content(&at_minimum, &format!("a removal naming {TAG_KEYS_MIN} key"));

    // An operation whose model states an explicit success code keeps the stated one.
    let deleted = emulator.delete_assistant(&host_assistant).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "case \"DeleteAssistant\": expected the status its model states, got {} with {}",
        deleted.status, deleted.body
    );
}

/// The account id the emulator built this ARN under, read out of the ARN itself so the
/// not-found rows name the same account as the resources that do exist.
fn emulator_account(resource_arn: &str) -> String {
    resource_arn
        .split(':')
        .nth(4)
        .unwrap_or_default()
        .to_string()
}
