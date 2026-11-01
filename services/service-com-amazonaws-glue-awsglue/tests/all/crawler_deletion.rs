// SPDX-License-Identifier: CC0-1.0

//! `DeleteCrawler`: the crawler the request names stops existing.
//!
//! RULE: `DeleteCrawler` "removes a specified crawler from the Glue Data Catalog, unless the
//! crawler state is RUNNING" — so it answers 200 and removes the WHOLE record the create
//! wrote (the crawler's tags are held on it), leaving every other crawler where it is; a
//! `Name` no crawler in this account has is the operation's modelled
//! `EntityNotFoundException`; and a crawler whose crawl is in progress is its modelled
//! `CrawlerRunningException` with the crawler still there.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The tables below differ along what the rule generalises over: which members the deleted
//! crawler carries (a schedule, tags, a description, a table prefix, which crawl target
//! kind), how many other crawlers the store holds beside it, which name the refused delete
//! addresses (one nothing holds, one differing only in case from one it does, one already
//! deleted), and which non-`READY` `CrawlerState` the crawler is in.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCrawlerInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, CrawlerEntry, CRAWLER_STATE_RUNNING, CRAWLER_STATE_STOPPING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The role every crawler here is created with.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-deletion";

/// The error code AWS Glue answers for a name it holds no crawler under, and the one it
/// answers for a crawler whose crawl is in progress — each the operation's own modelled
/// error, as the awsJson `__type` spells it.
const ENTITY_NOT_FOUND: &str = "EntityNotFoundException";
const CRAWLER_RUNNING: &str = "CrawlerRunningException";

/// Create one crawler from the properties a row declares, beside the name and role every
/// row's crawler carries.
async fn given_crawler(router: &MultiServiceRouter, name: &str, properties: &Value) {
    let mut body = json!({
        "Name": name,
        "Role": ROLE,
        "Targets": { "S3Targets": [{ "Path": format!("s3://crawler-bucket/{name}/") }] },
    });
    for (member, value) in properties.as_object().into_iter().flatten() {
        body[member.clone()] = value.clone();
    }
    ok(router, "CreateCrawler", &body).await;
}

/// The crawler names this account holds, sorted, so the comparison is about membership.
async fn held_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let page = ok(router, "ListCrawlers", &json!({})).await;
    let mut names: Vec<String> = page["CrawlerNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCrawlers must report CrawlerNames, got {page}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// The crawler the request names stops existing
// ---------------------------------------------------------------------------

/// Deleting a crawler removes it from the store: `GetCrawler` no longer resolves its name,
/// the listing no longer reports it, its tags are gone with it, and the crawlers beside it
/// are untouched.
///
/// The rows differ along which members the deleted crawler carries and how many other
/// crawlers the store holds beside it.
#[tokio::test]
async fn deleting_a_crawler_removes_it_and_leaves_every_other_crawler_where_it_is() {
    // (case, the properties the deleted crawler carries beyond its name, role and targets,
    // the names of the crawlers held beside it).
    let rows: Vec<(&str, Value, &[&str])> = vec![
        (
            "a bare crawler, the only one the account holds",
            json!({}),
            &[],
        ),
        (
            "a scheduled, tagged crawler with two others beside it",
            json!({
                "Schedule": "cron(15 12 * * ? *)",
                "Description": "crawls the landing zone",
                "TablePrefix": "raw_",
                "Tags": { "team": "data", "env": "prod" }
            }),
            &["cd-other-one", "cd-other-two"],
        ),
        (
            "a crawler over Amazon DynamoDB, with one other beside it",
            json!({
                "Targets": { "DynamoDBTargets": [{ "Path": "cd-sessions" }] },
                "DatabaseName": "cd_catalog"
            }),
            &["cd-other-three"],
        ),
    ];

    for (case, properties, others) in rows {
        let router = emulator();
        let name = "cd-deleted";
        given_crawler(&router, name, &properties).await;
        for other in others {
            given_crawler(&router, other, &json!({})).await;
        }

        let deleted = ok(&router, "DeleteCrawler", &json!({ "Name": name })).await;
        assert!(
            deleted.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: DeleteCrawler reports no members, got {deleted}"
        );

        let (status, gone) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetCrawler on the deleted crawler, got \
             {status} with body {gone}"
        );
        assert_eq!(
            error_code(&gone),
            ENTITY_NOT_FOUND,
            "case {case:?}: expected {ENTITY_NOT_FOUND} from GetCrawler, got {gone}"
        );

        // The tags the create stored are held ON the record, so they go with it: the tag read
        // of the crawler's ARN no longer resolves a crawler either.
        let arn = format!("arn:aws:glue:us-east-1:{ACCOUNT_ID}:crawler/{name}");
        let (status, untagged) = call(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetTags on the deleted crawler's ARN, got \
             {status} with body {untagged}"
        );

        let mut expected: Vec<String> = others.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(
            held_names(&router, case).await,
            expected,
            "case {case:?}: expected only the crawlers beside the deleted one to be held"
        );
        for other in others {
            let read = ok(&router, "GetCrawler", &json!({ "Name": other })).await;
            assert_eq!(
                read.pointer("/Crawler/Name").and_then(Value::as_str),
                Some(*other),
                "case {case:?}: expected {other:?} to be untouched, got {read}"
            );
        }

        // The delete is not idempotent: the operation models a not-found refusal, so a second
        // delete of the same name is that refusal rather than another success.
        let (status, again) = call(&router, "DeleteCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a second DeleteCrawler, got {status} with \
             body {again}"
        );
        assert_eq!(
            error_code(&again),
            ENTITY_NOT_FOUND,
            "case {case:?}: expected {ENTITY_NOT_FOUND} from a second DeleteCrawler, got {again}"
        );
    }
}

// ---------------------------------------------------------------------------
// A name no crawler has is refused
// ---------------------------------------------------------------------------

/// A `Name` this account holds no crawler under is the modelled `EntityNotFoundException`,
/// worded as the service words a crawler miss and carrying only the name the request asked
/// for.
///
/// The rows differ along which name is addressed: one nothing holds, one differing only in
/// case from a held one (a crawler's name is matched exactly, unlike a database's or a
/// table's), one carrying the held name as a prefix, and one that is only a space.
#[tokio::test]
async fn deleting_a_crawler_no_name_resolves_is_refused_as_not_found() {
    let router = emulator();
    let held = "cd-held";
    given_crawler(&router, held, &json!({})).await;

    let rows: Vec<(&str, &str)> = vec![
        ("a name nothing holds", "cd-absent"),
        ("the held name in upper case", "CD-HELD"),
        ("the held name with a suffix", "cd-held-2"),
        ("a name that is only a space", " "),
    ];

    for (case, name) in rows {
        let (status, refused) = call(&router, "DeleteCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteCrawler, got {status} with body \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            ENTITY_NOT_FOUND,
            "case {case:?}: expected {ENTITY_NOT_FOUND}, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Crawler entry with name {name} does not exist"),
            "case {case:?}: expected the service's own crawler-miss wording, got {refused}"
        );
    }

    // And the crawler the account does hold is still there: a refused delete removes nothing.
    assert_eq!(
        held_names(&router, "a refused delete removes nothing").await,
        vec![held.to_string()],
        "a refused delete must leave the held crawler where it is"
    );
}

// ---------------------------------------------------------------------------
// A crawl in progress refuses the delete
// ---------------------------------------------------------------------------

/// "unless the crawler state is RUNNING": a crawler with a run in progress is the modelled
/// `CrawlerRunningException`, and the crawler is still there afterwards.
///
/// The rows are the modelled `CrawlerState` values other than `READY` — `RUNNING` and
/// `STOPPING`, both of which are a run that has not finished — seeded onto the stored record,
/// this emulator having no implemented call that leaves a crawler in either state yet.
#[tokio::test]
async fn deleting_a_crawler_whose_crawl_is_in_progress_is_refused() {
    for (case, state) in [
        ("a crawl in progress", CRAWLER_STATE_RUNNING),
        ("a crawl that was asked to stop", CRAWLER_STATE_STOPPING),
    ] {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        let name = "cd-crawling";
        {
            let stored = engine.state::<ComAmazonawsGlueAwsglueState>();
            let mut store = stored.lock();
            let input = CreateCrawlerInput {
                name: name.to_string(),
                role: ROLE.to_string(),
                ..CreateCrawlerInput::default()
            };
            let mut entry =
                CrawlerEntry::from_create_input(&input, ACCOUNT_ID, SystemTime::UNIX_EPOCH);
            entry.state = state.to_string();
            store.crawlers.insert(name.to_string(), entry);
        }

        let (status, refused) = call(&router, "DeleteCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteCrawler, got {status} with body \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            CRAWLER_RUNNING,
            "case {case:?}: expected {CRAWLER_RUNNING}, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Crawler with name {name} is running"),
            "case {case:?}: expected the service's own crawler-running wording, got {refused}"
        );

        let read = ok(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            read.pointer("/Crawler/State").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected the refused delete to leave the crawler in {state:?}, got \
             {read}"
        );
    }
}
