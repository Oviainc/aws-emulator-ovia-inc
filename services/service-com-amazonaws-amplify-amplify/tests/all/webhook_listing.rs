// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListWebhooks`.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses, not an internal function's return value.
//!
//! The rules under test:
//!
//! 1. `ListWebhooks` REPORTS the webhooks persisted under the app its URI names and
//!    nothing else — each element being the record `GetWebhook` answers for that
//!    webhook, in ascending `webhookId` order — and it changes nothing a later caller
//!    sees.
//! 2. The page bound is `maxResults`: a page holds `min(bound, remaining)` webhooks,
//!    a caller that set no bound (member absent, or its modelled default of 0) gets
//!    the modelled maximum, and `nextToken` is the id of the first webhook the page
//!    did NOT return, so a paged walk visits the collection once in order.
//! 3. A `nextToken` this service could not have issued is refused with the modelled
//!    `BadRequestException`; one it could have issued resumes the scan at the first
//!    id at or after it, even when no webhook holds that id.
//! 4. An app this store does not hold is the same modelled 404 every other
//!    app-scoped operation answers.
//!
//! Each table spans the class the rule generalises over — how many webhooks exist,
//! which bound the caller set, where a token falls in the collection — rather than
//! one recorded request.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, Emulator, Reply};

/// The modelled `range` maximum of `ListWebhooks$maxResults`, which is also the page
/// a caller that set no bound gets
/// (`smithy_query(query="operation",operation="ListWebhooks")`).
const MODELLED_MAX_RESULTS: usize = 50;

/// `WebhookId`'s modelled `length` maximum (`length 0..255`, from
/// `smithy_query(query="shape",shape="Webhook")`), which is the longest value a
/// `nextToken` this operation issued can have.
const WEBHOOK_ID_MAX_LENGTH: usize = 255;

/// The `webhooks` member of a `ListWebhooks` reply, which is `@required`: the member
/// is present even on a page that holds nothing.
fn listed(label: &str, reply: &Reply) -> Vec<Value> {
    let listed = reply
        .body
        .get("webhooks")
        .unwrap_or_else(|| panic!("case {label:?}: ListWebhooks must carry `webhooks`: {reply:?}"));
    listed
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: `webhooks` must be a list, got {listed:?}"))
        .clone()
}

/// The ids of the webhooks one page returned, in the order the page returned them.
fn listed_ids(label: &str, reply: &Reply) -> Vec<String> {
    listed(label, reply)
        .iter()
        .map(|record| string_member(record, "webhookId"))
        .collect()
}

/// The `nextToken` a listing reply carries, if any.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create `count` webhooks on `app_id` and answer their ids IN THE ORDER
/// `ListWebhooks` must report them (ascending `webhookId`, the key the app's webhook
/// map is held under).
///
/// The branch names and descriptions vary across the created webhooks, so the records
/// a page carries are distinguishable from one another rather than all alike.
async fn webhooks_of(emulator: &Emulator, app_id: &str, count: usize) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for index in 0..count {
        let request = if index % 2 == 0 {
            json!({ "branchName": format!("branch-{index}") })
        } else {
            json!({
                "branchName": format!("feature/{index}"),
                "description": format!("trigger {index}"),
            })
        };
        let created = emulator.create_webhook(app_id, &request).await;
        assert_eq!(
            created.status, 200,
            "seeding webhook {index} on {app_id}: expected HTTP 200, got {created:?}"
        );
        let webhook_id = string_member(&created.member("webhook"), "webhookId");
        assert!(
            ids.insert(webhook_id.clone()),
            "seeding webhook {index} on {app_id}: expected a fresh id, got {webhook_id:?} twice"
        );
    }
    ids.into_iter().collect()
}

#[tokio::test]
async fn list_webhooks_reports_exactly_the_apps_persisted_webhooks() {
    let emulator = Emulator::new();

    // A second app holds webhooks throughout: a page is scoped to ONE app, so no
    // record of this one may ever appear in the other's listing.
    let bystander_app = emulator.app_id_of_new_app("webhook-listing-bystander").await;
    let bystander_ids = webhooks_of(&emulator, &bystander_app, 3).await;

    // The counts span none, one, a few and more than one page bound's worth.
    for count in [0_usize, 1, 2, 5, 13] {
        let label = format!("{count} webhooks");
        let app_id = emulator
            .app_id_of_new_app(&format!("webhook-listing-{count}"))
            .await;
        let expected = webhooks_of(&emulator, &app_id, count).await;

        let reply = emulator.list_webhooks(&app_id, "").await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200 from ListWebhooks, got {reply:?}"
        );
        assert_eq!(
            listed_ids(&label, &reply),
            expected,
            "case {label:?}: ListWebhooks must report every persisted webhook of the app, in \
             ascending webhookId order, got {reply:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {label:?}: a page that reached the end carries no nextToken, got {reply:?}"
        );

        // Each listed element is the record the implemented reader answers for that
        // webhook, so a listing and a read of the same webhook cannot drift.
        for record in listed(&label, &reply) {
            let webhook_id = string_member(&record, "webhookId");
            let fetched = emulator.get_webhook(&webhook_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?}: a listed webhook must be readable by its id, got {fetched:?}"
            );
            assert_eq!(
                fetched.member("webhook"),
                record,
                "case {label:?}: the listed webhook {webhook_id:?} must equal the record \
                 GetWebhook answers, got {fetched:?}"
            );
        }

        // Listing is a report: the next caller sees exactly what this one saw, and the
        // other app's listing is untouched.
        let again = emulator.list_webhooks(&app_id, "").await;
        assert_eq!(
            again.body, reply.body,
            "case {label:?}: listing webhooks must not change what a later caller sees, got \
             {again:?}"
        );
        let bystander = emulator.list_webhooks(&bystander_app, "").await;
        assert_eq!(
            listed_ids("bystander app", &bystander),
            bystander_ids,
            "case {label:?}: one app's listing must report no other app's webhooks, got \
             {bystander:?}"
        );
    }
}

#[tokio::test]
async fn list_webhooks_pages_the_collection_once_under_every_bound() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("webhook-listing-paging").await;
    let expected = webhooks_of(&emulator, &app_id, 13).await;
    let expected_set: BTreeSet<String> = expected.iter().cloned().collect();

    // The bounds span 1, a middle value, exactly the number of records, more than the
    // number of records, the modelled `range` maximum, and the two "caller set no
    // bound" cases: the member absent, and its modelled default of 0.
    let bounds: Vec<(&str, String, Option<usize>)> = vec![
        ("maxResults=1", "?maxResults=1".to_string(), Some(1)),
        ("maxResults=4", "?maxResults=4".to_string(), Some(4)),
        ("maxResults=13", "?maxResults=13".to_string(), Some(13)),
        ("maxResults=14", "?maxResults=14".to_string(), Some(14)),
        ("maxResults=50", "?maxResults=50".to_string(), Some(50)),
        ("maxResults absent", String::new(), None),
        ("maxResults=0", "?maxResults=0".to_string(), None),
    ];

    for (label, query, bound) in bounds {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let paged = match &token {
                Some(value) => format!("{query}{separator}nextToken={value}"),
                None => query.clone(),
            };
            let reply = emulator.list_webhooks(&app_id, &paged).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 from ListWebhooks, got {reply:?}"
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {label:?}: paging did not terminate after {pages} pages"
            );

            let page = listed_ids(label, &reply);
            let remaining = expected.len() - seen.len();
            let want = bound.unwrap_or(MODELLED_MAX_RESULTS).min(remaining);
            assert_eq!(
                page.len(),
                want,
                "case {label:?}: page {pages} must hold min(bound, remaining) = {want} \
                 webhooks, got {page:?}"
            );
            seen.extend(page);

            token = next_token(&reply);
            match &token {
                // The token is the id of the first webhook the page did NOT return.
                Some(value) => assert_eq!(
                    Some(value),
                    expected.get(seen.len()),
                    "case {label:?}: the nextToken must name the first webhook not returned, \
                     got {value:?}"
                ),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {label:?}: a paged walk must visit the whole collection once, in order, got \
             {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            expected_set,
            "case {label:?}: a paged walk must visit every webhook of the app"
        );
    }

    // One past the modelled maximum is refused by the boundary validator, not served.
    let refused = emulator.list_webhooks(&app_id, "?maxResults=51").await;
    assert_eq!(
        refused.status, 400,
        "case \"maxResults=51\": a value past the modelled range maximum must be refused, got \
         {refused:?}"
    );
}

#[tokio::test]
async fn list_webhooks_refuses_a_token_it_never_issued_and_resumes_at_one_it_did() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("webhook-listing-tokens").await;
    let expected = webhooks_of(&emulator, &app_id, 5).await;

    // Every token this operation issues is a `webhookId`, modelled `length 0..255`,
    // while `NextToken` admits up to 2000 characters: a longer value could not be an
    // id this service issued, so it is refused rather than served as an arbitrary
    // suffix of the collection.
    for (label, token) in [
        ("token one character past the id maximum", "t".repeat(WEBHOOK_ID_MAX_LENGTH + 1)),
        ("token at the nextToken maximum", "t".repeat(2000)),
    ] {
        let reply = emulator
            .list_webhooks(&app_id, &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: an unissued nextToken must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?}",
            reply.error_code
        );
    }

    // A token the service could have issued resumes the scan at the first id AT OR
    // AFTER it — including an id the app does not hold, which is how a page boundary
    // survives a collection that changed between two calls. The ids are hexadecimal
    // UUIDs, so "0" sorts at or below every one of them and "g" above all of them.
    let resumptions: Vec<(String, String, usize)> = vec![
        ("token naming the first webhook".to_string(), expected[0].clone(), 0),
        ("token naming a middle webhook".to_string(), expected[2].clone(), 2),
        ("token naming the last webhook".to_string(), expected[4].clone(), 4),
        ("token naming no webhook, below them all".to_string(), "0".to_string(), 0),
        ("token naming no webhook, above them all".to_string(), "g".to_string(), 5),
        (
            "token at the id maximum, above them all".to_string(),
            "g".repeat(WEBHOOK_ID_MAX_LENGTH),
            5,
        ),
    ];
    for (label, token, skipped) in resumptions {
        let reply = emulator
            .list_webhooks(&app_id, &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200 from ListWebhooks, got {reply:?}"
        );
        assert_eq!(
            listed_ids(&label, &reply),
            expected[skipped..].to_vec(),
            "case {label:?}: the page must resume at the first id at or after the token, got \
             {reply:?}"
        );
    }
}

#[tokio::test]
async fn list_webhooks_refuses_an_app_the_store_does_not_hold() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("webhook-listing-refusal").await;
    let expected = webhooks_of(&emulator, &app_id, 2).await;

    // `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`, so each of these is a
    // well-formed id for an app that does not exist — the request passes the boundary
    // validator and the refusal is the handler's.
    for absent_app in ["dnosuchapp1", "d0", &format!("d{}", "9".repeat(18))] {
        let refused = emulator.list_webhooks(absent_app, "").await;
        assert_eq!(
            refused.status, 404,
            "case {absent_app:?}: expected HTTP 404 from ListWebhooks, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("NotFoundException"),
            "case {absent_app:?}: expected NotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            Some(format!("App {absent_app} not found.")),
            "case {absent_app:?}: the 404 names the entity the request addressed, got {:?}",
            refused.message()
        );
    }

    // The refusals changed nothing, and an app whose webhooks went with it is itself
    // gone: deleting the app makes its listing the same modelled 404.
    assert_eq!(
        listed_ids("surviving app", &emulator.list_webhooks(&app_id, "").await),
        expected,
        "a refused listing must leave the real app's webhooks exactly as they were"
    );
    let deleted = emulator.delete_app(&app_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteApp, got {deleted:?}"
    );
    let gone = emulator.list_webhooks(&app_id, "").await;
    assert_eq!(
        gone.status, 404,
        "the webhooks of a deleted app are gone with it, got {gone:?}"
    );
}
