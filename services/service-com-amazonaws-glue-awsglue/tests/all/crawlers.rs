// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue crawler slice: `CreateCrawler` and the `GetCrawler`
//! that reads what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The suites are tables: each row is one input along the axis the rule generalises
//! over (which target lists are filled, which optional members are sent, the form of
//! the schedule expression, the casing of the name), and the recorded CLI case — a
//! name, a role ARN, a database and one `S3Targets` path — is one row among them.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCrawlerInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, CrawlerEntry, CRAWLER_STATE_READY, FIRST_CRAWLER_VERSION,
    MAX_CRAWLERS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, ok, ACCOUNT_ID};

/// A `CrawlerTargets` naming one Amazon S3 prefix — the target the recorded CLI
/// workflow sends, and the smallest input that satisfies "at least one crawl target".
fn s3_targets(path: &str) -> Value {
    json!({ "S3Targets": [{ "Path": path }] })
}

/// A minimal `CreateCrawler` body: the three required members and nothing else.
fn crawler_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
        "Targets": s3_targets("s3://crawler-bucket/data/"),
    })
}

/// Create `name` with the minimal body and assert it succeeded.
async fn create_crawler(router: &MultiServiceRouter, name: &str) {
    ok(router, "CreateCrawler", &crawler_body(name)).await;
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn get_crawler(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetCrawler", &json!({ "Name": name })).await;
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawler must report a Crawler, got {body}"))
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateCrawler` persists is what `GetCrawler` reports: every member the request
/// carried round-trips unchanged, and the members only the server sets (`State`,
/// `Version`, `CreationTime`, `LastUpdated`, `CatalogId`) are filled from the store.
///
/// The rows differ along the member set sent and along which of the eight target lists
/// `CrawlerTargets` carries is filled: the bare required members (the recorded CLI
/// case), a fully-specified crawler with a schedule and policies, a JDBC target, a
/// DynamoDB target, a Data Catalog target and an Iceberg target.
#[tokio::test]
async fn created_crawlers_are_read_back_from_the_store() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            // The recorded workflow's own shape: name, role ARN, database, one S3 path.
            "required members only",
            json!({
                "Name": "sim2real-crawler",
                "Role": "arn:aws:iam::000000000000:role/sim2real-crawler-role",
                "DatabaseName": "sim2real_crawler_db",
                "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
            }),
        ),
        (
            "every optional member",
            json!({
                "Name": "fully-specified",
                "Role": "GlueCrawlerRole",
                "DatabaseName": "analytics",
                "Description": "crawls the curated prefix",
                "Targets": {
                    "S3Targets": [{
                        "Path": "s3://curated/events/",
                        "Exclusions": ["**/_SUCCESS", "**/*.tmp"],
                        "SampleSize": 42,
                    }],
                },
                "Schedule": "cron(15 12 * * ? *)",
                "Classifiers": ["csv-classifier", "json-classifier"],
                "TablePrefix": "curated_",
                "SchemaChangePolicy": {
                    "UpdateBehavior": "LOG",
                    "DeleteBehavior": "DEPRECATE_IN_DATABASE",
                },
                "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_NEW_FOLDERS_ONLY" },
                "LineageConfiguration": { "CrawlerLineageSettings": "ENABLE" },
                "LakeFormationConfiguration": {
                    "UseLakeFormationCredentials": true,
                    "AccountId": ACCOUNT_ID,
                },
                "Configuration": "{\"Version\":1.0,\"Grouping\":{\"TableGroupingPolicy\":\"CombineCompatibleSchemas\"}}",
                "CrawlerSecurityConfiguration": "crawler-security",
                "CatalogId": ACCOUNT_ID,
            }),
        ),
        (
            "jdbc target",
            json!({
                "Name": "jdbc-crawler",
                "Role": "GlueCrawlerRole",
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
            "dynamodb target",
            json!({
                "Name": "dynamodb-crawler",
                "Role": "GlueCrawlerRole",
                "Targets": {
                    "DynamoDBTargets": [{ "Path": "orders" }],
                },
            }),
        ),
        (
            "catalog target",
            json!({
                "Name": "catalog-crawler",
                "Role": "GlueCrawlerRole",
                "Targets": {
                    "CatalogTargets": [{
                        "DatabaseName": "analytics",
                        "Tables": ["events", "orders"],
                    }],
                },
            }),
        ),
        (
            "iceberg target",
            json!({
                "Name": "iceberg-crawler",
                "Role": "GlueCrawlerRole",
                "Targets": {
                    "IcebergTargets": [{
                        "Paths": ["s3://lake/iceberg/"],
                        "MaximumTraversalDepth": 3,
                    }],
                },
            }),
        ),
    ];

    for (case, request) in &cases {
        let created_at = SystemTime::now();
        ok(&router, "CreateCrawler", request).await;
        let crawler = get_crawler(
            &router,
            request["Name"].as_str().expect("the case names a crawler"),
        )
        .await;

        // Everything the request carried, except the members that are not part of the
        // `Crawler` shape (`Tags`) or that the server re-shapes (`Schedule`).
        let mut echoed = request.clone();
        let sent_schedule = echoed
            .as_object_mut()
            .expect("the request is a structure")
            .remove("Schedule");
        assert_echoed(case, &echoed, &crawler);

        // A crawler is created ready to run: no crawl is in progress, none has run.
        assert_eq!(
            crawler.get("State").and_then(Value::as_str),
            Some(CRAWLER_STATE_READY),
            "case {case}: expected State {CRAWLER_STATE_READY}, got {crawler}"
        );
        assert_eq!(
            crawler.get("Version").and_then(Value::as_i64),
            Some(FIRST_CRAWLER_VERSION),
            "case {case}: expected Version {FIRST_CRAWLER_VERSION}, got {crawler}"
        );
        assert_eq!(
            crawler.get("LastCrawl"),
            None,
            "case {case}: a crawler that never ran reports no LastCrawl, got {crawler}"
        );
        assert_eq!(
            crawler.get("CatalogId").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {case}: expected CatalogId {ACCOUNT_ID}, got {crawler}"
        );
        // `Classifiers` is a list of the custom classifiers associated with the
        // crawler, so a crawler created without any reports an empty list.
        let expected_classifiers = request
            .get("Classifiers")
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            crawler.get("Classifiers"),
            Some(&expected_classifiers),
            "case {case}: expected Classifiers {expected_classifiers}, got {crawler}"
        );

        // A `Schedule` cron expression is reported inside a `Schedule` object that is
        // SCHEDULED; a crawler created without one has no schedule at all.
        match sent_schedule {
            Some(expression) => assert_eq!(
                crawler.get("Schedule"),
                Some(&json!({ "ScheduleExpression": expression, "State": "SCHEDULED" })),
                "case {case}: expected the sent cron expression, SCHEDULED, got {crawler}"
            ),
            None => assert_eq!(
                crawler.get("Schedule"),
                None,
                "case {case}: a crawler with no schedule reports none, got {crawler}"
            ),
        }

        // The server-set timestamps are wall-clock instants from the create, equal to
        // each other because nothing has updated the crawler since.
        let creation_time = crawler
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime, got {crawler}"));
        let after = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs_f64();
        let before = created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs_f64();
        assert!(
            (before.floor()..=after.ceil()).contains(&creation_time),
            "case {case}: expected a CreationTime within [{before}, {after}], got {creation_time}"
        );
        assert_eq!(
            crawler.get("LastUpdated").and_then(Value::as_f64),
            Some(creation_time),
            "case {case}: an un-updated crawler's LastUpdated is its CreationTime, got {crawler}"
        );
    }

    // Each create is its own record: every crawler of the table is still readable.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case names a crawler");
        let crawler = get_crawler(&router, name).await;
        assert_eq!(
            crawler.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: {name} must still be readable after the later creates"
        );
    }
}

/// Reading a crawler changes nothing a later caller sees: two identical `GetCrawler`
/// calls answer byte-for-byte the same crawler, and the read does not move its state.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = emulator();
    create_crawler(&router, "idempotent-crawler").await;

    let first = get_crawler(&router, "idempotent-crawler").await;
    let second = get_crawler(&router, "idempotent-crawler").await;
    assert_eq!(
        first, second,
        "case repeated GetCrawler: expected {first}, got {second}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// `CreateCrawler` documents that at least one crawl target must be specified. A
/// `Targets` that names no data store — whatever list it carries — is refused with
/// `InvalidInputException`, and a `Targets` with any one list non-empty is accepted.
///
/// The rows span the empty forms (no list at all, and each list present but empty) and
/// the accepted forms (one entry in each of the eight target lists).
#[tokio::test]
async fn a_create_naming_no_crawl_target_is_refused() {
    let router = emulator();

    // Every target list `CrawlerTargets` carries, from the model's shape.
    let lists = [
        ("S3Targets", json!({ "Path": "s3://lake/data/" })),
        ("JdbcTargets", json!({ "ConnectionName": "warehouse" })),
        ("MongoDBTargets", json!({ "ConnectionName": "mongo" })),
        ("DynamoDBTargets", json!({ "Path": "orders" })),
        (
            "CatalogTargets",
            json!({ "DatabaseName": "analytics", "Tables": ["events"] }),
        ),
        ("DeltaTargets", json!({ "DeltaTables": ["s3://lake/delta/"] })),
        ("IcebergTargets", json!({ "Paths": ["s3://lake/iceberg/"] })),
        ("HudiTargets", json!({ "Paths": ["s3://lake/hudi/"] })),
    ];

    // Refused: no list at all, and each list present but empty.
    let mut empty_cases: Vec<(String, Value)> = vec![("no target list".to_string(), json!({}))];
    for (list, _) in &lists {
        let mut targets = serde_json::Map::new();
        targets.insert((*list).to_string(), json!([]));
        empty_cases.push((format!("empty {list}"), Value::Object(targets)));
    }
    // And every list present, all empty.
    let mut all_empty = serde_json::Map::new();
    for (list, _) in &lists {
        all_empty.insert((*list).to_string(), json!([]));
    }
    empty_cases.push(("every list empty".to_string(), Value::Object(all_empty)));

    for (index, (case, targets)) in empty_cases.iter().enumerate() {
        let mut request = crawler_body(&format!("empty-targets-{index}"));
        request["Targets"] = targets.clone();
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        // A refused create leaves nothing behind.
        let (status, body) = call(
            &router,
            "GetCrawler",
            &json!({ "Name": format!("empty-targets-{index}") }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: the refused crawler must not be stored, got {status} with {body}"
        );
    }

    // Accepted: one entry in any one of the lists.
    for (list, entry) in &lists {
        let case = format!("one {list}");
        let mut request = crawler_body(&format!("one-{list}"));
        let mut targets = serde_json::Map::new();
        targets.insert((*list).to_string(), json!([entry]));
        request["Targets"] = Value::Object(targets);
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
    }
}

/// A `Schedule` is "a cron expression used to specify the schedule", of the documented
/// form `cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)`. A value
/// of that form is stored and reported back; anything else is refused with
/// `InvalidInputException` rather than stored as a schedule nothing can run.
///
/// The rows span the accepted forms (the documented example, a wildcard-only
/// expression, named day-of-week and month fields, a step and a range) and the refused
/// ones (a bare cron body, a rate expression, too few and too many fields, an unclosed
/// expression and the empty string).
#[tokio::test]
async fn a_schedule_must_be_a_cron_expression() {
    let router = emulator();

    let accepted = [
        // The model's own example, on `Schedule$ScheduleExpression`.
        "cron(15 12 * * ? *)",
        "cron(* * * * ? *)",
        "cron(0 10 ? * MON-FRI *)",
        "cron(0/15 * * * ? *)",
        "cron(0 0 1 JAN ? 2027)",
    ];
    for (index, expression) in accepted.iter().enumerate() {
        let mut request = crawler_body(&format!("scheduled-{index}"));
        request["Schedule"] = json!(expression);
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 200,
            "case accepted {expression}: expected 200, got {status} with {body}"
        );
        let crawler = get_crawler(&router, &format!("scheduled-{index}")).await;
        assert_eq!(
            crawler.get("Schedule"),
            Some(&json!({ "ScheduleExpression": expression, "State": "SCHEDULED" })),
            "case accepted {expression}: expected the expression reported as SCHEDULED, got {crawler}"
        );
    }

    let refused = [
        "15 12 * * ? *",
        "rate(5 minutes)",
        "cron(15 12 * * ?)",
        "cron(15 12 * * ? * *)",
        "cron(15 12 * * ? *",
        "",
    ];
    for (index, expression) in refused.iter().enumerate() {
        let name = format!("unscheduled-{index}");
        let mut request = crawler_body(&name);
        request["Schedule"] = json!(expression);
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 400,
            "case refused {expression:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case refused {expression:?}: expected InvalidInputException, got {body}"
        );
        let (status, body) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case refused {expression:?}: the refused crawler must not be stored, got {status} with {body}"
        );
    }
}

/// Addressing a crawler that is not stored is `EntityNotFoundException`, and creating
/// one whose name is already taken is `AlreadyExistsException` — both HTTP 400, and
/// neither changes the stored record.
///
/// The rows span absence (never created, and a name that differs only in case, because
/// the model documents no case fold for a crawler name) and the duplicate create.
#[tokio::test]
async fn absent_and_duplicate_crawlers_answer_the_modelled_errors() {
    let router = emulator();
    create_crawler(&router, "present-crawler").await;

    for (case, name) in [
        ("never created", "absent-crawler"),
        ("differs only in case", "PRESENT-CRAWLER"),
        ("differs only in one letter's case", "present-Crawler"),
    ] {
        let (status, body) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
    }

    // A name that differs only in case is a DIFFERENT crawler, and creating it works.
    ok(&router, "CreateCrawler", &crawler_body("PRESENT-CRAWLER")).await;
    for name in ["present-crawler", "PRESENT-CRAWLER"] {
        let crawler = get_crawler(&router, name).await;
        assert_eq!(
            crawler.get("Name").and_then(Value::as_str),
            Some(name),
            "case casing {name}: expected its own record, got {crawler}"
        );
    }

    // The duplicate create is refused, and leaves the stored crawler as it was.
    let stored = get_crawler(&router, "present-crawler").await;
    let mut duplicate = crawler_body("present-crawler");
    duplicate["Description"] = json!("a second create must not overwrite the first");
    let (status, body) = call(&router, "CreateCrawler", &duplicate).await;
    assert_eq!(
        status, 400,
        "case duplicate create: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("AlreadyExistsException"),
        "case duplicate create: expected AlreadyExistsException, got {body}"
    );
    assert_eq!(
        get_crawler(&router, "present-crawler").await,
        stored,
        "case duplicate create: the stored crawler must be unchanged, expected {stored}"
    );
}

/// A `CatalogId` this emulator does not hold names no Data Catalog here, so a create
/// that writes its output there is refused; the account's own id, named or omitted,
/// reaches the one catalog this emulator holds.
#[tokio::test]
async fn catalog_id_selects_the_one_catalog_this_emulator_holds() {
    let router = emulator();

    for (case, catalog_id) in [("omitted", None), ("named", Some(ACCOUNT_ID))] {
        let name = format!("own-catalog-{case}");
        let mut request = crawler_body(&name);
        if let Some(catalog_id) = catalog_id {
            request["CatalogId"] = json!(catalog_id);
        }
        ok(&router, "CreateCrawler", &request).await;
        let crawler = get_crawler(&router, &name).await;
        assert_eq!(
            crawler.get("CatalogId").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case own catalog {case}: expected CatalogId {ACCOUNT_ID}, got {crawler}"
        );
    }

    for foreign in ["123456789012", "999999999999"] {
        let mut request = crawler_body("foreign-catalog");
        request["CatalogId"] = json!(foreign);
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 400,
            "case foreign catalog {foreign}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case foreign catalog {foreign}: expected InvalidInputException, got {body}"
        );
    }
}

/// The store holds a bounded number of crawlers: the create at the ceiling succeeds,
/// the one past it is refused with the modelled limit error, and deleting nothing —
/// just failing — leaves the ceiling a bound rather than a one-way latch, so a create
/// succeeds again once a slot is free.
#[tokio::test]
async fn a_create_past_the_crawler_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_CRAWLERS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateCrawlerInput {
                name: name.clone(),
                role: "GlueCrawlerRole".to_string(),
                ..CreateCrawlerInput::default()
            };
            let entry = CrawlerEntry::from_create_input(&input, ACCOUNT_ID, SystemTime::UNIX_EPOCH);
            store.crawlers.insert(name, entry);
        }
    }

    ok(&router, "CreateCrawler", &crawler_body("last-crawler")).await;
    let (status, body) = call(&router, "CreateCrawler", &crawler_body("one-too-many")).await;
    assert_eq!(
        status, 400,
        "case crawler {} of {MAX_CRAWLERS}: expected 400, got {status} with {body}",
        MAX_CRAWLERS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one crawler too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    // Free one slot and the create succeeds again: the ceiling is a bound on what is
    // stored, not a latch on how many creates were ever made.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().crawlers.remove("seeded-0");
    }
    ok(&router, "CreateCrawler", &crawler_body("one-too-many")).await;
}
