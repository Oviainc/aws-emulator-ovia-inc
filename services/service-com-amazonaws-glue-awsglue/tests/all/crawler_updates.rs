// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCrawler`, and the `GetCrawler` that reads back what it
//! wrote.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `UpdateCrawler` merges the request into the stored crawler —
//! every member the request CARRIES replaces the stored one, every member it OMITS is
//! preserved ("if you omit this value, the existing value on the crawler is preserved",
//! `UpdateCrawlerRequest$CatalogId`), an empty `Description` takes the description off
//! (that member targets the REMOVABLE description shape where the create targets the
//! plain one), and the write moves the crawler's `Version` and `LastUpdated`. A crawler
//! with a crawl in progress is not updatable, and a request whose members would leave
//! behind a crawler no create could make is refused without touching what is stored.
//!
//! The recorded workflow's own call — one crawler, updated by naming a new schedule — is
//! ONE row of the first table; the rows differ along which members the request names,
//! how many it names, whether a named member replaces, fills or clears what is stored,
//! and which kind of target list it moves the crawler to.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, CRAWLER_STATE_READY, CRAWLER_STATE_RUNNING,
    CRAWLER_STATE_STOPPING, FIRST_CRAWLER_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The role the stored crawler carries before every update here.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-before";

/// The role an update may move the crawler to, so a row that names a role is observably
/// a different value from the one the create stored.
const UPDATED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-v2";

/// The crawler each row creates BEFORE updating it: every member a `CreateCrawler`
/// request can carry, so a member the update does not name is observably preserved
/// rather than merely absent from both records.
fn crawler_before_update(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "DatabaseName": "analytics_before",
        "Description": "the crawler before the update",
        "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/before/" }] },
        "Schedule": "cron(15 12 * * ? *)",
        "Classifiers": ["csv-classifier"],
        "TablePrefix": "before_",
        "SchemaChangePolicy": {
            "UpdateBehavior": "LOG",
            "DeleteBehavior": "DEPRECATE_IN_DATABASE",
        },
        "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_EVERYTHING" },
        "LineageConfiguration": { "CrawlerLineageSettings": "DISABLE" },
        "LakeFormationConfiguration": {
            "UseLakeFormationCredentials": false,
            "AccountId": ACCOUNT_ID,
        },
        "Configuration": "{\"Version\":1.0,\"Grouping\":{\"TableGroupingPolicy\":\"CombineCompatibleSchemas\"}}",
        "CrawlerSecurityConfiguration": "crawler-security-before",
        "Tags": { "owner": "data-platform" },
    })
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn get_crawler(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetCrawler", &json!({ "Name": name })).await;
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawler must report a Crawler, got {body}"))
}

/// Create the fully-specified crawler of [`crawler_before_update`] and report what
/// `GetCrawler` reads back for it.
async fn create_crawler(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "CreateCrawler", &crawler_before_update(name)).await;
    get_crawler(router, name).await
}

/// The `Crawler` a merge of `update` onto `before` must produce, for every member except
/// the two the write moves (`Version` and `LastUpdated`), which each test asserts itself.
///
/// This is the rule, applied member by member rather than row by row: a named member
/// replaces, an unnamed one is left standing, an empty `Description` is taken off, and a
/// cron expression is reported inside the `Schedule` structure that carries its state.
fn merged(before: &Value, update: &Value) -> Value {
    let mut expected = before.clone();
    let members = expected.as_object_mut().expect("a Crawler is a structure");
    for (member, value) in update.as_object().expect("an update is a structure") {
        match member.as_str() {
            // `Name` addresses the crawler; the request has no member that renames one.
            "Name" => {}
            // The removable description: an empty one takes the description off.
            "Description" if value == &json!("") => {
                members.remove("Description");
            }
            "Schedule" => {
                members.insert(
                    "Schedule".to_string(),
                    json!({ "ScheduleExpression": value, "State": "SCHEDULED" }),
                );
            }
            _ => {
                members.insert(member.clone(), value.clone());
            }
        }
    }
    expected
}

/// The `LastUpdated` of a reported crawler, as epoch seconds.
fn last_updated(case: &str, crawler: &Value) -> f64 {
    crawler
        .get("LastUpdated")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: expected a LastUpdated, got {crawler}"))
}

/// Epoch seconds of a wall-clock instant, for bounding a server-set timestamp.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// The merge
// ---------------------------------------------------------------------------

/// An update replaces the members it names, preserves the ones it omits, and moves the
/// crawler's `Version` and `LastUpdated` — whatever subset of the request's members it
/// carries.
///
/// The rows differ along which members are named and how many: the recorded workflow's
/// own shape (a new schedule alone), a single scalar, the removable description cleared
/// and replaced, a classifier list emptied, a move to a different kind of crawl target,
/// the policies, the catalog, and every member at once.
#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_preserves_the_rest() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            // The recorded workflow's own call: one crawler, given a new schedule.
            "schedule alone",
            json!({ "Schedule": "cron(0 1 * * ? *)" }),
        ),
        ("role alone", json!({ "Role": UPDATED_ROLE_ARN })),
        (
            "description replaced",
            json!({ "Description": "the crawler after the update" }),
        ),
        ("description cleared", json!({ "Description": "" })),
        ("classifiers emptied", json!({ "Classifiers": [] })),
        (
            "classifiers replaced",
            json!({ "Classifiers": ["json-classifier", "grok-classifier"] }),
        ),
        (
            "targets moved to another kind",
            json!({
                "Targets": {
                    "JdbcTargets": [{
                        "ConnectionName": "warehouse",
                        "Path": "inventory/%/%",
                        "Exclusions": ["inventory/staging/%"],
                    }],
                },
            }),
        ),
        (
            "database and table prefix",
            json!({ "DatabaseName": "analytics_after", "TablePrefix": "after_" }),
        ),
        (
            "the policies",
            json!({
                "SchemaChangePolicy": {
                    "UpdateBehavior": "UPDATE_IN_DATABASE",
                    "DeleteBehavior": "DELETE_FROM_DATABASE",
                },
                "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_NEW_FOLDERS_ONLY" },
                "LineageConfiguration": { "CrawlerLineageSettings": "ENABLE" },
                "LakeFormationConfiguration": {
                    "UseLakeFormationCredentials": true,
                    "AccountId": ACCOUNT_ID,
                },
            }),
        ),
        (
            "configuration and security configuration",
            json!({
                "Configuration": "{\"Version\":1.0,\"CrawlerOutput\":{\"Partitions\":{\"AddOrUpdateBehavior\":\"InheritFromTable\"}}}",
                "CrawlerSecurityConfiguration": "crawler-security-after",
            }),
        ),
        // Naming the account's own catalog changes nothing observable, but it IS a
        // member the request carried, so the write still happens.
        ("catalog named", json!({ "CatalogId": ACCOUNT_ID })),
        (
            "every member",
            json!({
                "Role": UPDATED_ROLE_ARN,
                "DatabaseName": "analytics_after",
                "Description": "every member at once",
                "Targets": { "DynamoDBTargets": [{ "Path": "orders" }] },
                "Schedule": "cron(30 2 ? * MON-FRI *)",
                "Classifiers": ["xml-classifier"],
                "TablePrefix": "after_",
                "SchemaChangePolicy": {
                    "UpdateBehavior": "UPDATE_IN_DATABASE",
                    "DeleteBehavior": "LOG",
                },
                "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_NEW_FOLDERS_ONLY" },
                "LineageConfiguration": { "CrawlerLineageSettings": "ENABLE" },
                "LakeFormationConfiguration": {
                    "UseLakeFormationCredentials": true,
                    "AccountId": ACCOUNT_ID,
                },
                "Configuration": "{\"Version\":1.0}",
                "CrawlerSecurityConfiguration": "crawler-security-after",
                "CatalogId": ACCOUNT_ID,
            }),
        ),
    ];

    for (index, (case, update)) in cases.iter().enumerate() {
        let name = format!("merge-{index}");
        let before = create_crawler(&router, &name).await;

        let mut request = update.clone();
        request["Name"] = json!(name);
        let started = SystemTime::now();
        let answer = ok(&router, "UpdateCrawler", &request).await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: UpdateCrawler answers an empty structure, got {answer}"
        );

        let after = get_crawler(&router, &name).await;

        // The version moves on by exactly one write.
        let expected_version = before
            .get("Version")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: expected a Version, got {before}"))
            + 1;
        assert_eq!(
            after.get("Version").and_then(Value::as_i64),
            Some(expected_version),
            "case {case}: expected Version {expected_version}, got {after}"
        );

        // `LastUpdated` is the instant of THIS write, so it is at or after the call and
        // no earlier than what the create left; `CreationTime` does not move at all.
        let moved = last_updated(case, &after);
        let floor = epoch_seconds(started).floor();
        let ceiling = epoch_seconds(SystemTime::now()).ceil();
        assert!(
            (floor..=ceiling).contains(&moved),
            "case {case}: expected a LastUpdated within [{floor}, {ceiling}], got {moved}"
        );
        assert!(
            moved >= last_updated(case, &before),
            "case {case}: LastUpdated must not go backwards, got {moved} after {}",
            last_updated(case, &before)
        );
        assert_eq!(
            after.get("CreationTime"),
            before.get("CreationTime"),
            "case {case}: an update does not move CreationTime, got {after}"
        );

        // Everything else is the merge: named members replaced, the rest preserved.
        let mut expected = merged(&before, &request);
        expected["Version"] = json!(expected_version);
        expected["LastUpdated"] = after["LastUpdated"].clone();
        assert_eq!(
            after, expected,
            "case {case}: expected {expected}, got {after}"
        );
    }
}

/// A request that names only the crawler asks for no change: nothing is written, so the
/// stored record — its `Version` and `LastUpdated` included — is exactly what it was.
#[tokio::test]
async fn an_update_naming_only_the_crawler_changes_nothing() {
    let router = emulator();
    let before = create_crawler(&router, "untouched").await;

    for round in 0..3 {
        ok(&router, "UpdateCrawler", &json!({ "Name": "untouched" })).await;
        let after = get_crawler(&router, "untouched").await;
        assert_eq!(
            after, before,
            "case round {round}: a request naming no member changes nothing, expected \
             {before}, got {after}"
        );
    }
}

/// Each update that changes something advances the version by one, from the version the
/// create left: the counter follows the writes, not the calls.
#[tokio::test]
async fn each_write_advances_the_version_by_one() {
    let router = emulator();
    let before = create_crawler(&router, "versioned").await;
    assert_eq!(
        before.get("Version").and_then(Value::as_i64),
        Some(FIRST_CRAWLER_VERSION),
        "case fresh crawler: expected Version {FIRST_CRAWLER_VERSION}, got {before}"
    );

    for round in 1..=4_i64 {
        ok(
            &router,
            "UpdateCrawler",
            &json!({ "Name": "versioned", "TablePrefix": format!("round_{round}_") }),
        )
        .await;
        // A call that asks for no change writes nothing, so it does not advance it.
        ok(&router, "UpdateCrawler", &json!({ "Name": "versioned" })).await;

        let crawler = get_crawler(&router, "versioned").await;
        let expected = FIRST_CRAWLER_VERSION + round;
        assert_eq!(
            crawler.get("Version").and_then(Value::as_i64),
            Some(expected),
            "case round {round}: expected Version {expected}, got {crawler}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Updating a crawler this account does not hold is the modelled not-found refusal, and
/// it creates nothing: the rows span a name never created and names that differ from a
/// stored crawler's only in case, because the model documents no case fold for a crawler
/// name.
#[tokio::test]
async fn an_update_of_an_absent_crawler_is_the_modelled_miss() {
    let router = emulator();
    create_crawler(&router, "present-crawler").await;

    for (case, name) in [
        ("never created", "absent-crawler"),
        ("differs only in case", "PRESENT-CRAWLER"),
        ("differs only in one letter's case", "present-Crawler"),
    ] {
        let (status, body) = call(
            &router,
            "UpdateCrawler",
            &json!({ "Name": name, "TablePrefix": "nowhere_" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::crawler_not_found(name),
            "case {case}: expected the crawler miss for {name}, got {body}"
        );

        // The refused update created nothing under that name.
        let (status, body) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused update must not create a crawler, got {status} with {body}"
        );
    }
}

/// A request whose members would leave behind a crawler no create could make is refused
/// with the modelled input error, and the stored crawler — its `Version` included — is
/// untouched.
///
/// The rows span the three rules the update carries over from the create: a `Targets`
/// naming no data store (no list at all, and a list present but empty), a `Schedule`
/// that is not a `cron(..)` expression of six fields, and a blank `Role`; plus a
/// `CatalogId` naming a Data Catalog this emulator does not hold.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_crawler_unchanged() {
    let router = emulator();
    let stored = create_crawler(&router, "unchanged").await;

    let refused: Vec<(&str, Value)> = vec![
        ("no target list", json!({ "Targets": {} })),
        ("empty S3Targets", json!({ "Targets": { "S3Targets": [] } })),
        (
            "every target list empty",
            json!({
                "Targets": {
                    "S3Targets": [],
                    "JdbcTargets": [],
                    "MongoDBTargets": [],
                    "DynamoDBTargets": [],
                    "CatalogTargets": [],
                    "DeltaTargets": [],
                    "IcebergTargets": [],
                    "HudiTargets": [],
                },
            }),
        ),
        ("bare cron body", json!({ "Schedule": "15 12 * * ? *" })),
        ("a rate expression", json!({ "Schedule": "rate(5 minutes)" })),
        ("too few cron fields", json!({ "Schedule": "cron(15 12 * * ?)" })),
        (
            "too many cron fields",
            json!({ "Schedule": "cron(15 12 * * ? * *)" }),
        ),
        ("unclosed cron", json!({ "Schedule": "cron(15 12 * * ? *" })),
        ("the empty schedule", json!({ "Schedule": "" })),
        ("the empty role", json!({ "Role": "" })),
        ("a blank role", json!({ "Role": "   " })),
        ("another account's catalog", json!({ "CatalogId": "123456789012" })),
        ("a catalog no create made", json!({ "CatalogId": "999999999999" })),
    ];

    for (case, update) in &refused {
        let mut request = update.clone();
        request["Name"] = json!("unchanged");
        // A member that WOULD be applied rides along, so a refusal that wrote the rest
        // of the request would be visible.
        request["Description"] = json!("this update must not land");
        let (status, body) = call(&router, "UpdateCrawler", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        let after = get_crawler(&router, "unchanged").await;
        assert_eq!(
            after, stored,
            "case {case}: the stored crawler must be unchanged, expected {stored}, got {after}"
        );
    }
}

/// "If a crawler is running, you must stop it using StopCrawler before updating it":
/// `READY` is the one modelled `CrawlerState` with no run in progress, so an update is
/// refused in every other state and the stored crawler is untouched.
///
/// The crawler is put into each state directly, because the operations that move it
/// there are not implemented yet; the rule under test is the refusal, not how the state
/// got there.
#[tokio::test]
async fn a_crawler_with_a_crawl_in_progress_is_not_updatable() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    create_crawler(&router, "busy").await;

    /// Put the stored crawler into `state`.
    fn set_state(engine: &Arc<engine::InMemoryEngine>, name: &str, state: &str) {
        let store = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = store.lock();
        let entry = store
            .crawlers
            .get_mut(name)
            .unwrap_or_else(|| panic!("the suite created {name}"));
        entry.state = state.to_string();
    }

    for case in [CRAWLER_STATE_RUNNING, CRAWLER_STATE_STOPPING] {
        set_state(&engine, "busy", case);
        let stored = get_crawler(&router, "busy").await;

        let (status, body) = call(
            &router,
            "UpdateCrawler",
            &json!({ "Name": "busy", "Description": "not while a crawl runs" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case state {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("CrawlerRunningException"),
            "case state {case}: expected CrawlerRunningException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::crawler_running("busy"),
            "case state {case}: expected the running refusal for busy, got {body}"
        );
        let after = get_crawler(&router, "busy").await;
        assert_eq!(
            after, stored,
            "case state {case}: the stored crawler must be unchanged, expected {stored}, got \
             {after}"
        );
    }

    // Back to the state a crawler with no run in progress is in, and the same update
    // lands: the refusal is about the run, not about the request.
    set_state(&engine, "busy", CRAWLER_STATE_READY);
    ok(
        &router,
        "UpdateCrawler",
        &json!({ "Name": "busy", "Description": "the crawl is over" }),
    )
    .await;
    let after = get_crawler(&router, "busy").await;
    assert_eq!(
        after.get("Description").and_then(Value::as_str),
        Some("the crawl is over"),
        "case state {CRAWLER_STATE_READY}: expected the update to land, got {after}"
    );
}
