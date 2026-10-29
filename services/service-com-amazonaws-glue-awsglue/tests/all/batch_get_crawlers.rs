// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetCrawlers`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `BatchGetCrawlers` answers 200 and reports, for EACH name in
//! `CrawlerNames` and in the order the request listed them, either the crawler stored
//! under that name — the same projection `GetCrawler` reports — or that name in
//! `CrawlersNotFound`; a name no crawler holds is not an error, and the call changes
//! nothing a later caller sees. The tables below span the batch along what the rule
//! generalises over: how many names are asked for, how many of them are stored, where
//! the misses sit in the order, whether a name repeats, and the member set the stored
//! crawler carries.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCrawlerInput;
use service_com_amazonaws_glue_awsglue::state::{ComAmazonawsGlueAwsglueState, CrawlerEntry};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok, ACCOUNT_ID};

/// The largest `CrawlerNames` list the model accepts.
///
/// Source: `smithy_query(query="shape",shape="CrawlerNameList")`, whose `length` is
/// `0..100`; the same bound is on the response's `CrawlersNotFound`.
const MAX_CRAWLER_NAMES: usize = 100;

/// The role every crawler here is created with.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-batch";

/// A minimal `CreateCrawler` body: the three required members plus a description, so each
/// stored crawler carries a member that distinguishes it from its siblings.
fn crawler_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE,
        "Targets": { "S3Targets": [{ "Path": format!("s3://crawler-bucket/{name}/") }] },
        "Description": format!("crawler {name}"),
    })
}

/// One `BatchGetCrawlers` call that must succeed, as
/// `(the reported Crawlers' names, CrawlersNotFound)`.
async fn batch_get(router: &MultiServiceRouter, names: &[&str]) -> (Vec<String>, Vec<String>) {
    let body = ok(router, "BatchGetCrawlers", &json!({ "CrawlerNames": names })).await;
    (crawler_names(&body), string_list(&body, "CrawlersNotFound"))
}

/// The `Name` of every element of the response's `Crawlers` list, in order.
fn crawler_names(body: &Value) -> Vec<String> {
    crate::wire::names_in(body, "Crawlers")
}

/// The strings of a top-level list member, which must be present even when empty.
fn string_list(body: &Value, member: &str) -> Vec<String> {
    crate::wire::string_list(body, member)
}

// ---------------------------------------------------------------------------
// The batch splits the requested names across the two lists
// ---------------------------------------------------------------------------

/// One row of the batch table: the case name, the names the request asks for, the names
/// the response must report in `Crawlers`, and the names it must report in
/// `CrawlersNotFound`.
type BatchCase<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);

/// Each requested name is reported in exactly one of the two lists, in the order asked.
///
/// The rows differ along the batch's size, how many of its names are stored, where the
/// misses sit (first, last, interleaved), and whether a name repeats. The recorded
/// reference case — one batch naming a crawler the workflow just created — is the "one
/// stored name" row.
#[tokio::test]
async fn every_requested_name_is_reported_exactly_once() {
    let router = emulator();
    for stored in ["alpha", "beta", "gamma"] {
        ok(&router, "CreateCrawler", &crawler_body(stored)).await;
    }

    // (case, requested names, expected Crawlers names, expected CrawlersNotFound)
    let cases: Vec<BatchCase> = vec![
        // The recorded case: a batch naming the single crawler under test.
        ("one stored name", vec!["alpha"], vec!["alpha"], vec![]),
        (
            "every name stored",
            vec!["alpha", "beta", "gamma"],
            vec!["alpha", "beta", "gamma"],
            vec![],
        ),
        (
            "no name stored",
            vec!["absent", "missing"],
            vec![],
            vec!["absent", "missing"],
        ),
        (
            "the miss comes first",
            vec!["absent", "beta"],
            vec!["beta"],
            vec!["absent"],
        ),
        (
            "the miss comes last",
            vec!["beta", "absent"],
            vec!["beta"],
            vec!["absent"],
        ),
        (
            "misses interleaved with hits",
            vec!["absent", "alpha", "missing", "gamma", "nope"],
            vec!["alpha", "gamma"],
            vec!["absent", "missing", "nope"],
        ),
        (
            // A crawler name carries no documented case fold, so a different casing is
            // a different crawler — and so a miss.
            "a stored name in another casing",
            vec!["ALPHA", "alpha"],
            vec!["alpha"],
            vec!["ALPHA"],
        ),
        (
            // One answer per requested name, not per distinct name.
            "a repeated stored name",
            vec!["beta", "beta"],
            vec!["beta", "beta"],
            vec![],
        ),
        (
            "a repeated unknown name",
            vec!["absent", "absent"],
            vec![],
            vec!["absent", "absent"],
        ),
        (
            // A batch with nothing in it: both lists are present and empty. `0` is the
            // modelled minimum length of `CrawlerNameList`.
            "an empty batch",
            vec![],
            vec![],
            vec![],
        ),
    ];

    for (case, requested, expected_crawlers, expected_missing) in cases {
        let body = ok(
            &router,
            "BatchGetCrawlers",
            &json!({ "CrawlerNames": requested }),
        )
        .await;

        let reported = crawler_names(&body);
        assert_eq!(
            reported, expected_crawlers,
            "case {case:?}: expected Crawlers {expected_crawlers:?}, got {reported:?} in {body}"
        );
        let missing = string_list(&body, "CrawlersNotFound");
        assert_eq!(
            missing, expected_missing,
            "case {case:?}: expected CrawlersNotFound {expected_missing:?}, got {missing:?} in \
             {body}"
        );
        // The two lists together account for every requested name, once each.
        assert_eq!(
            reported.len() + missing.len(),
            requested.len(),
            "case {case:?}: every requested name must be reported exactly once, got {body}"
        );

        // A read does not change what the next caller sees: the same batch answers the
        // same thing, and each name is still readable through `GetCrawler`.
        let again = ok(
            &router,
            "BatchGetCrawlers",
            &json!({ "CrawlerNames": requested }),
        )
        .await;
        assert_eq!(
            again, body,
            "case {case:?}: BatchGetCrawlers must be repeatable, got {again}"
        );
        for stored in ["alpha", "beta", "gamma"] {
            let read = ok(&router, "GetCrawler", &json!({ "Name": stored })).await;
            assert_eq!(
                read.get("Crawler").and_then(|crawler| crawler.get("Name")),
                Some(&json!(stored)),
                "case {case:?}: the batch read must not have touched {stored:?}, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What it reports is what the store holds
// ---------------------------------------------------------------------------

/// A crawler the batch reports is byte-for-byte the definition `GetCrawler` reports for
/// the same name — the batch is a different addressing mode over one stored resource, not
/// a second projection of it.
///
/// The rows differ along the member set the create carried, so the comparison covers a
/// bare crawler, a scheduled one, one with a non-S3 target and a fully specified one.
#[tokio::test]
async fn a_batched_crawler_is_the_crawler_get_crawler_reports() {
    let router = emulator();

    let creates: Vec<(&str, Value)> = vec![
        ("required members only", crawler_body("bare")),
        (
            "a scheduled crawler",
            json!({
                "Name": "scheduled",
                "Role": ROLE,
                "Targets": { "S3Targets": [{ "Path": "s3://lake/events/" }] },
                "Schedule": "cron(15 12 * * ? *)",
            }),
        ),
        (
            "a jdbc target",
            json!({
                "Name": "jdbc",
                "Role": ROLE,
                "Targets": {
                    "JdbcTargets": [{ "ConnectionName": "warehouse", "Path": "inventory/%/%" }],
                },
            }),
        ),
        (
            "every optional member this batch cares about",
            json!({
                "Name": "full",
                "Role": ROLE,
                "DatabaseName": "analytics",
                "Description": "crawls the curated prefix",
                "Targets": {
                    "S3Targets": [{
                        "Path": "s3://curated/events/",
                        "Exclusions": ["**/_SUCCESS"],
                        "SampleSize": 42,
                    }],
                },
                "Classifiers": ["csv-classifier"],
                "TablePrefix": "curated_",
                "SchemaChangePolicy": { "UpdateBehavior": "LOG" },
                "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_NEW_FOLDERS_ONLY" },
                "LineageConfiguration": { "CrawlerLineageSettings": "ENABLE" },
                "CatalogId": ACCOUNT_ID,
                "Tags": { "owner": "data-platform" },
            }),
        ),
    ];

    let mut names = Vec::new();
    for (case, request) in &creates {
        ok(&router, "CreateCrawler", request).await;
        let name = request
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: every row names its crawler"));
        names.push(name.to_string());
    }

    let requested: Vec<&str> = names.iter().map(String::as_str).collect();
    let body = ok(
        &router,
        "BatchGetCrawlers",
        &json!({ "CrawlerNames": requested }),
    )
    .await;
    let reported = body
        .get("Crawlers")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("BatchGetCrawlers must report a Crawlers list, got {body}"))
        .clone();

    for ((case, _), name) in creates.iter().zip(names.iter()) {
        let single = ok(&router, "GetCrawler", &json!({ "Name": name })).await;
        let expected = single.get("Crawler").unwrap_or_else(|| {
            panic!("case {case:?}: GetCrawler must report a Crawler, got {single}")
        });
        let actual = reported
            .iter()
            .find(|crawler| crawler.get("Name") == Some(&json!(name)))
            .unwrap_or_else(|| panic!("case {case:?}: the batch must report {name:?}, got {body}"));
        assert_eq!(
            actual, expected,
            "case {case:?}: the batched crawler must be the crawler GetCrawler reports, got \
             {actual}"
        );
        // `Tags` is not a member of the `Crawler` shape, so a batched crawler carries
        // none.
        assert!(
            actual.get("Tags").is_none(),
            "case {case:?}: Tags is not a member of Crawler, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled bounds are refused, not reported as a miss
// ---------------------------------------------------------------------------

/// A name outside the modelled `length 1..255`, and a batch past the modelled
/// `CrawlerNames` length of 100, are refused at the request boundary rather than reported
/// as misses — neither addresses a crawler, so each is a bad request — and the
/// `@required` member itself must be present.
///
/// Source for both bounds: `smithy_query(query="shape",shape="CrawlerNameList")` — the
/// list is `length 0..100` and its member targets `NameString`, `length 1..255`. Each
/// published bound is exercised, plus one past it.
#[tokio::test]
async fn names_outside_the_modeled_bounds_are_refused() {
    let router = emulator();
    ok(&router, "CreateCrawler", &crawler_body("alpha")).await;

    // At each published element bound, the name is a legal address — so it is simply
    // unknown, and reported as a miss rather than refused.
    for length in [1usize, 255] {
        let name = "n".repeat(length);
        let body = ok(
            &router,
            "BatchGetCrawlers",
            &json!({ "CrawlerNames": [name.clone()] }),
        )
        .await;
        assert_eq!(
            string_list(&body, "CrawlersNotFound"),
            vec![name.clone()],
            "case name of {length} characters: a legal but unknown name is a miss, got {body}"
        );
        assert!(
            crawler_names(&body).is_empty(),
            "case name of {length} characters: no crawler is stored under it, got {body}"
        );
    }

    // One past each bound is refused: an element outside its length, and a batch of one
    // name too many.
    let over: Vec<String> = (0..=MAX_CRAWLER_NAMES)
        .map(|index| format!("absent-{index}"))
        .collect();
    for (case, names) in [
        ("an empty name", json!([""])),
        ("a name of 256 characters", json!(["n".repeat(256)])),
        (
            "a legal name beside an over-long one",
            json!(["alpha", "n".repeat(256)]),
        ),
        ("one name too many", json!(over)),
    ] {
        let (status, answer) = call(&router, "BatchGetCrawlers", &json!({ "CrawlerNames": names }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            !error_code(&answer).is_empty(),
            "case {case:?}: the refusal must name an error, got {answer}"
        );
    }

    // `CrawlerNames` is `@required`, so a batch that names it not at all is refused too.
    let (status, answer) = call(&router, "BatchGetCrawlers", &json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no CrawlerNames member\": expected 400, got {status} with {answer}"
    );
    assert!(
        !error_code(&answer).is_empty(),
        "case \"no CrawlerNames member\": the refusal must name an error, got {answer}"
    );
}

/// A batch AT the modelled ceiling is served in full, whatever its names are: the bound
/// is a bound, not a smaller page.
///
/// The rows differ along the batch's size (one, a middling batch, the ceiling exactly)
/// and along what the names are: distinct stored crawlers, one stored name repeated, and
/// unknown names.
#[tokio::test]
async fn a_batch_at_the_modeled_ceiling_is_served_in_full() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Enough distinct crawlers to fill a batch at the ceiling. Seeded directly: filling
    // the store over the wire would exercise the create path, which has its own suite.
    let stored: Vec<String> = (0..MAX_CRAWLER_NAMES)
        .map(|index| format!("batched-{index:03}"))
        .collect();
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for name in &stored {
            let input = CreateCrawlerInput {
                name: name.clone(),
                role: ROLE.to_string(),
                ..CreateCrawlerInput::default()
            };
            let entry = CrawlerEntry::from_create_input(&input, ACCOUNT_ID, SystemTime::UNIX_EPOCH);
            store.crawlers.insert(name.clone(), entry);
        }
    }

    for size in [1usize, 25, MAX_CRAWLER_NAMES] {
        let distinct: Vec<&str> = stored.iter().take(size).map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &distinct).await;
        assert_eq!(
            found, distinct,
            "case {size} distinct stored names: expected them all reported, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} distinct stored names: none may miss, got {missing:?}"
        );

        let repeated: Vec<&str> = vec!["batched-000"; size];
        let (found, missing) = batch_get(&router, &repeated).await;
        assert_eq!(
            found.len(),
            size,
            "case {size} copies of one stored name: one entry per requested name, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} copies of one stored name: none may miss, got {missing:?}"
        );

        let unknown: Vec<String> = (0..size).map(|index| format!("absent-{index}")).collect();
        let unknown: Vec<&str> = unknown.iter().map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &unknown).await;
        assert!(
            found.is_empty(),
            "case {size} unknown names: nothing may be reported, got {found:?}"
        );
        assert_eq!(
            missing, unknown,
            "case {size} unknown names: all of them miss, got {missing:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A crawler that leaves the store becomes a miss
// ---------------------------------------------------------------------------

/// The batch reports what the store holds AT THE TIME OF THE CALL: a name moves from
/// `Crawlers` to `CrawlersNotFound` when its crawler leaves the store, and back when it
/// is created again.
///
/// The crawler leaves the store directly rather than through `DeleteCrawler`, which is
/// still a scaffold in this crate; what is under test is that the batch reads the store
/// on every call rather than caching its first answer.
#[tokio::test]
async fn the_batch_follows_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    ok(&router, "CreateCrawler", &crawler_body("transient")).await;

    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"created\": a stored crawler is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"created\": a stored crawler is not missing, got {missing:?}"
    );

    engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .crawlers
        .remove("transient");
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert!(
        found.is_empty(),
        "case \"removed\": a crawler no longer stored is not reported, got {found:?}"
    );
    assert_eq!(
        missing,
        vec!["transient".to_string()],
        "case \"removed\": a crawler no longer stored is reported missing, got {missing:?}"
    );

    ok(&router, "CreateCrawler", &crawler_body("transient")).await;
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"re-created\": a re-created crawler is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"re-created\": a re-created crawler is not missing, got {missing:?}"
    );
}
