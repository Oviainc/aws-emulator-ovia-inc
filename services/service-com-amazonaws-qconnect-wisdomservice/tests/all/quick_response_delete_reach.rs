// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `DeleteQuickResponse` REMOVES, driven over HTTP through the
//! same router the emulator serves with.
//!
//! The rule under test, asserted over TABLES of inputs rather than the one recorded
//! request: a delete removes the stored record itself, so afterwards NO read path of this
//! service reports it — neither its own `Get`, nor its knowledge base's listing or search,
//! nor the cross-family tagging surface its ARN reaches — and the `clientToken` that
//! created it no longer answers with it, while every other record of the same knowledge
//! base is left exactly as it was.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * the path a later caller comes back through: `GetQuickResponse`, `ListQuickResponses`,
//!   `SearchQuickResponses` (the whole knowledge base, and a query naming the record),
//!   `ListTagsForResource` and `TagResource` on the record's own ARN;
//! * which record of the knowledge base the delete names (a middle one, the first of what
//!   is left, the last one) and how many are left behind (several, one, none);
//! * for the token rows, what the create replayed after the delete carries: the same body,
//!   other content, or another name and shortcut key.
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass. The harness is the one this surface is already driven with:
//! the router and reply reader from `resource_tags.rs`, and the quick-response calls from
//! `quick_response_mutations.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::quick_response_mutations::{
    create_quick_response, delete_quick_response, get_quick_response, knowledge_base,
    list_quick_responses, CONTENT_TYPE_PLAIN, MESSAGE_QUICK_RESPONSE_NOT_FOUND,
};
use super::resource_tags::{
    label, Emulator, Reply, MESSAGE_TAGGING_RESOURCE_NOT_FOUND, TAGGING_NOT_FOUND_CODE,
};

/// Where plain-text content is reported inside the union-valued `contents`
/// (`smithy_query(query="shape",shape="QuickResponseContents")`).
const CONTENT_POINTER: &str = "/quickResponse/contents/plainText/content";

/// A create body with the members every row below reads back, including a tag so the
/// tagging surface has something to report for the record's ARN.
fn create_body(name: &str, shortcut_key: &str, content: &str) -> Value {
    json!({
        "name": name,
        "content": { "content": content },
        "contentType": CONTENT_TYPE_PLAIN,
        "description": format!("the {name} reply"),
        "shortcutKey": shortcut_key,
        "isActive": true,
        "channels": ["Chat"],
        "language": "en_US",
        "tags": { "owner": name },
    })
}

async fn search_quick_responses(
    emulator: &Emulator,
    knowledge_base: &str,
    expression: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/search/quickResponses",
                label(knowledge_base)
            ),
            Some(json!({ "searchExpression": expression })),
        )
        .await
}

/// A search expression that selects the records whose `name` contains `name` — the
/// queryable field the model documents
/// (`smithy_query(query="doc",shape="QuickResponseQueryField")`: content, name,
/// description, shortcutKey).
fn by_name(name: &str) -> Value {
    json!({ "queries": [{ "name": "name", "values": [name], "operator": "CONTAINS" }] })
}

/// `TagResource` on one resource ARN.
async fn tag_resource(emulator: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// The `quickResponseId`s a listing carries, as a set: what the rows are about is WHICH
/// records a read reports, which is independent of the order they come back in.
fn listed_ids(case: &str, reply: &Reply) -> BTreeSet<String> {
    ids_under(case, reply, "/quickResponseSummaries")
}

/// The `quickResponseId`s one search answered, as a set.
fn hit_ids(case: &str, reply: &Reply) -> BTreeSet<String> {
    ids_under(case, reply, "/results")
}

fn ids_under(case: &str, reply: &Reply, pointer: &str) -> BTreeSet<String> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .field(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} is required, got {}", reply.body))
        .iter()
        .map(|entry| {
            entry
                .get("quickResponseId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: an entry carries no id, got {entry}"))
                .to_string()
        })
        .collect()
}

/// One stored quick response, as the rows below address it.
#[derive(Debug, Clone)]
struct Record {
    case: &'static str,
    name: String,
    id: String,
    arn: String,
}

/// Create one record and answer the labels every read path below reaches it by.
async fn seed(
    emulator: &Emulator,
    knowledge_base: &str,
    case: &'static str,
    name: &str,
    shortcut_key: &str,
) -> Record {
    let created = create_quick_response(
        emulator,
        knowledge_base,
        create_body(name, shortcut_key, &format!("The {name} reply text")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the record this row deletes must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    Record {
        case,
        name: name.to_string(),
        id: created.string("/quickResponse/quickResponseId"),
        arn: created.string("/quickResponse/quickResponseArn"),
    }
}

/// Every read path REPORTS the record: its own `Get`, its knowledge base's listing, a
/// search of the whole knowledge base, a search naming it, and the tagging surface its ARN
/// reaches.
async fn assert_reported(emulator: &Emulator, knowledge_base: &str, record: &Record, case: &str) {
    let read = get_quick_response(emulator, knowledge_base, &record.id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected the record to be readable, got HTTP {} with body {}",
        read.status, read.body
    );
    assert_eq!(
        read.string("/quickResponse/name"),
        record.name,
        "case {case:?}: expected the stored name, got {}",
        read.body
    );

    let listing = list_quick_responses(emulator, knowledge_base).await;
    assert!(
        listed_ids(case, &listing).contains(&record.id),
        "case {case:?}: expected the listing to carry {}, got {}",
        record.id,
        listing.body
    );

    let whole = search_quick_responses(emulator, knowledge_base, json!({})).await;
    assert!(
        hit_ids(case, &whole).contains(&record.id),
        "case {case:?}: expected a search of the knowledge base to carry {}, got {}",
        record.id,
        whole.body
    );

    let named = search_quick_responses(emulator, knowledge_base, by_name(&record.name)).await;
    assert!(
        hit_ids(case, &named).contains(&record.id),
        "case {case:?}: expected a search for its name to select {}, got {}",
        record.id,
        named.body
    );

    let tags = emulator.list_tags(&record.arn).await;
    assert_eq!(
        tags.status, 200,
        "case {case:?}: expected the tags of {} to be readable, got HTTP {} with body {}",
        record.arn, tags.status, tags.body
    );
    assert_eq!(
        tags.string("/tags/owner"),
        record.name,
        "case {case:?}: expected the tag the create persisted, got {}",
        tags.body
    );
}

/// No read path reports the record any more, and the two that address it DIRECTLY answer
/// the class each of those surfaces words a resource it does not hold with.
async fn assert_gone(emulator: &Emulator, knowledge_base: &str, record: &Record, case: &str) {
    let read = get_quick_response(emulator, knowledge_base, &record.id).await;
    assert_eq!(
        read.status, 404,
        "case {case:?}: expected HTTP 404 from a read after the delete, got {} with body {}",
        read.status, read.body
    );
    assert!(
        read.error_type.contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?} with body {}",
        read.error_type,
        read.body
    );
    assert_eq!(
        read.string("/message"),
        MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        "case {case:?}: expected the sentence this class is worded with, got {}",
        read.body
    );
    assert_eq!(
        read.string("/resourceName"),
        record.id,
        "case {case:?}: the refusal names the resource the request addressed, got {}",
        read.body
    );

    let listing = list_quick_responses(emulator, knowledge_base).await;
    assert!(
        !listed_ids(case, &listing).contains(&record.id),
        "case {case:?}: the listing still carries the deleted {}, got {}",
        record.id,
        listing.body
    );

    let whole = search_quick_responses(emulator, knowledge_base, json!({})).await;
    assert!(
        !hit_ids(case, &whole).contains(&record.id),
        "case {case:?}: a search of the knowledge base still carries the deleted {}, got {}",
        record.id,
        whole.body
    );

    let named = search_quick_responses(emulator, knowledge_base, by_name(&record.name)).await;
    assert!(
        !hit_ids(case, &named).contains(&record.id),
        "case {case:?}: a search for the deleted record's name still selects {}, got {}",
        record.id,
        named.body
    );

    // The tagging surface takes an ARN rather than a path label, so the record's own ARN is
    // a second way back to it: with the record gone it names no resource of this account.
    for (surface, reply) in [
        ("ListTagsForResource", emulator.list_tags(&record.arn).await),
        (
            "TagResource",
            tag_resource(emulator, &record.arn, json!({ "owner": "someone-else" })).await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {case:?} ({surface}): expected HTTP 404 for the deleted record's ARN, got {} \
             with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains(TAGGING_NOT_FOUND_CODE),
            "case {case:?} ({surface}): expected {TAGGING_NOT_FOUND_CODE}, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_TAGGING_RESOURCE_NOT_FOUND,
            "case {case:?} ({surface}): expected the sentence this surface is worded with, got {}",
            reply.body
        );
    }
}

/// Which record of the still-live set a row deletes.
#[derive(Debug, Clone, Copy)]
enum Position {
    First,
    Middle,
    Last,
}

impl Position {
    /// The index this position names in a live set of `live` records.
    fn index(self, live: usize) -> usize {
        match self {
            Position::First => 0,
            Position::Middle => live / 2,
            Position::Last => live - 1,
        }
    }
}

/// RULE: a delete removes the record itself, so every read path of the service stops
/// reporting it while every other record of the same knowledge base is left alone.
#[tokio::test]
async fn a_delete_stops_every_read_path_from_reporting_the_record() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "quick-response-delete-reach").await;

    // Three records whose names are neither prefixes nor substrings of one another, so a
    // search naming one selects exactly that one.
    let mut live = vec![
        seed(&emulator, &knowledge_base_id, "alpha", "reply-alpha", "/a").await,
        seed(&emulator, &knowledge_base_id, "bravo", "reply-bravo", "/b").await,
        seed(
            &emulator,
            &knowledge_base_id,
            "charlie",
            "reply-charlie",
            "/c",
        )
        .await,
    ];

    // Rows: WHICH record of the live set the delete names, and so how many are left
    // behind — two, one, none. The label is only for the messages; the row's position is
    // what picks the record.
    for (position, case_label) in [
        (Position::Middle, "a middle record"),
        (Position::First, "the first of what is left"),
        (Position::Last, "the last record"),
    ] {
        let record = live.remove(position.index(live.len()));
        let case = format!("{case_label} ({})", record.case);

        // Before the delete every path reports it, so what the row asserts afterwards is
        // the delete's effect rather than a path that never answered.
        assert_reported(
            &emulator,
            &knowledge_base_id,
            &record,
            &format!("{case}, before the delete"),
        )
        .await;

        let deleted = delete_quick_response(&emulator, &knowledge_base_id, &record.id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: the delete's output declares no member, so its body carries none, \
             got {}",
            deleted.body
        );

        assert_gone(&emulator, &knowledge_base_id, &record, &case).await;
        for kept in &live {
            assert_reported(
                &emulator,
                &knowledge_base_id,
                kept,
                &format!("{case}, {} left alone", kept.case),
            )
            .await;
        }
    }
}

/// How the create replayed after the delete differs from the one the token first created.
#[derive(Debug, Clone, Copy)]
enum Replayed {
    SameBody,
    OtherContent,
    OtherNameAndShortcutKey,
}

/// RULE: the `clientToken` that created a record identifies a LIVE one, so once the record
/// is deleted the same token no longer answers with it: the create it carries is a fresh
/// one, whatever body it asks for.
#[tokio::test]
async fn a_client_token_whose_quick_response_was_deleted_does_not_resurrect_it() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "quick-response-delete-tokens").await;

    for (index, (case, replayed)) in [
        ("the same body again", Replayed::SameBody),
        ("a body carrying other content", Replayed::OtherContent),
        (
            "a body carrying another name and shortcut key",
            Replayed::OtherNameAndShortcutKey,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let token = format!("token-of-{index}");
        let name = format!("token-reply-{index}");
        let shortcut_key = format!("/t{index}");
        let content = format!("The first {name} reply");
        let mut first = create_body(&name, &shortcut_key, &content);
        first["clientToken"] = json!(token);

        let created = create_quick_response(&emulator, &knowledge_base_id, first.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create this row replays must succeed, got HTTP {} with body {}",
            created.status, created.body
        );
        let created_id = created.string("/quickResponse/quickResponseId");

        // While the record is live the token is a retry: it answers with what it already
        // created rather than a second record.
        let retried = create_quick_response(&emulator, &knowledge_base_id, first).await;
        assert_eq!(
            retried.string("/quickResponse/quickResponseId"),
            created_id,
            "case {case:?}: a live token must answer with the record it already created, got \
             HTTP {} with body {}",
            retried.status,
            retried.body
        );

        let deleted = delete_quick_response(&emulator, &knowledge_base_id, &created_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        let (replay_name, replay_shortcut_key, replay_content) = match replayed {
            Replayed::SameBody => (name.clone(), shortcut_key.clone(), content.clone()),
            Replayed::OtherContent => (
                name.clone(),
                shortcut_key.clone(),
                format!("Another {name} reply"),
            ),
            Replayed::OtherNameAndShortcutKey => (
                format!("{name}-renamed"),
                format!("/r{index}"),
                content.clone(),
            ),
        };
        let mut replay = create_body(&replay_name, &replay_shortcut_key, &replay_content);
        replay["clientToken"] = json!(token);
        let remade = create_quick_response(&emulator, &knowledge_base_id, replay).await;
        assert_eq!(
            remade.status, 200,
            "case {case:?}: a token whose record is gone must carry a fresh create, got HTTP {} \
             with body {}",
            remade.status, remade.body
        );
        let remade_id = remade.string("/quickResponse/quickResponseId");
        assert_ne!(
            remade_id, created_id,
            "case {case:?}: the replayed token must not answer with the deleted record, got {}",
            remade.body
        );
        assert_eq!(
            remade.string("/quickResponse/name"),
            replay_name,
            "case {case:?}: the fresh record must carry the replay's own name, got {}",
            remade.body
        );
        assert_eq!(
            remade.string(CONTENT_POINTER),
            replay_content,
            "case {case:?}: the fresh record must carry the replay's own content, got {}",
            remade.body
        );

        // And the deleted record is still gone: the replay created a second resource, it
        // did not revive the first.
        let read = get_quick_response(&emulator, &knowledge_base_id, &created_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the deleted record to stay gone, got HTTP {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("/message"),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
            "case {case:?}: expected the sentence this class is worded with, got {}",
            read.body
        );
    }
}
