// SPDX-License-Identifier: CC0-1.0

//! `DeleteCrawler`: what removing a crawler record reaches, and what it must leave alone.
//!
//! RULE: `DeleteCrawler` "removes a specified crawler from the Glue Data Catalog" — the
//! WHOLE record held under that name and nothing else. Two consequences are what this
//! suite pins: the name the removed crawler occupied is FREE again, so a later create
//! under it succeeds and the crawler it stores carries only the members ITS request named
//! (no description, schedule, table prefix, database, classifier list or tags of the
//! removed record, and the first definition version again); and no other resource of the
//! account moves, not even one held under the SAME name, because a crawler is addressed
//! "by name alone" in the crawler namespace only.
//!
//! [`crate::crawler_deletion`] covers the removal itself and the operation's two
//! refusals; this suite covers what must be true of the store AFTER one.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The tables below differ along what the rule generalises over: which members the removed
//! crawler carried (none, every member a create can name, tags over a JDBC target), and —
//! for the namespace table — which other resource kind holds the name and what form that
//! name takes.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{CRAWLER_STATE_READY, FIRST_CRAWLER_VERSION};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, ok, string_list, ACCOUNT_ID};

/// The role the removed crawler of each row carries.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-scope";

/// The role the crawler created under the freed name carries, so the second record is
/// observably the second request's and not a survival of the first.
const REPLACEMENT_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-replacement";

/// The one crawl target the replacement create names.
const REPLACEMENT_PATH: &str = "s3://crawler-bucket/replacement/";

/// The modelled errors this suite branches on, as the awsJson `__type` spells them.
const ALREADY_EXISTS: &str = "AlreadyExistsException";
const ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

/// Every member of `Crawler` that a create can FILL and that the model leaves absent when
/// no create named it (`smithy_query shape=Crawler`): a crawler created without them must
/// report none of them, so a value here after a delete-then-create is a record that
/// outlived its deletion.
const OPTIONAL_CRAWLER_MEMBERS: [&str; 9] = [
    "Description",
    "Schedule",
    "TablePrefix",
    "DatabaseName",
    "SchemaChangePolicy",
    "RecrawlPolicy",
    "LineageConfiguration",
    "Configuration",
    "CrawlerSecurityConfiguration",
];

/// Create one crawler under `name`, carrying the members a row declares beside the name,
/// role and target every row's crawler has.
async fn create_crawler(router: &MultiServiceRouter, name: &str, properties: &Value) {
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

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn get_crawler(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let body = ok(router, "GetCrawler", &json!({ "Name": name })).await;
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetCrawler must report a Crawler, got {body}"))
}

// ---------------------------------------------------------------------------
// The name a removed crawler held is free again
// ---------------------------------------------------------------------------

/// A crawler name is taken while the crawler is held and free once it is removed, and the
/// crawler created under the freed name starts from its own request: the removed record
/// leaves nothing behind — not a member, not a tag, not a definition version.
///
/// The rows differ along which members the removed crawler carried.
#[tokio::test]
async fn a_deleted_crawler_frees_its_name_and_the_next_create_inherits_nothing() {
    // (case, the members the removed crawler carries beyond its name, role and target).
    let rows: Vec<(&str, Value)> = vec![
        ("a bare crawler", json!({})),
        (
            "a crawler carrying every member a create can name",
            json!({
                "DatabaseName": "scope_catalog",
                "Description": "the crawler that was removed",
                "Schedule": "cron(15 12 * * ? *)",
                "Classifiers": ["scope-classifier"],
                "TablePrefix": "removed_",
                "SchemaChangePolicy": {
                    "UpdateBehavior": "LOG",
                    "DeleteBehavior": "DEPRECATE_IN_DATABASE",
                },
                "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_EVERYTHING" },
                "LineageConfiguration": { "CrawlerLineageSettings": "ENABLE" },
                "Configuration": "{\"Version\":1.0}",
                "CrawlerSecurityConfiguration": "scope-security",
                "Tags": { "team": "data", "env": "prod" },
            }),
        ),
        (
            "a tagged crawler over a JDBC target",
            json!({
                "Targets": {
                    "JdbcTargets": [{ "ConnectionName": "warehouse", "Path": "sales/%" }],
                },
                "Tags": { "owner": "platform" },
            }),
        ),
    ];

    for (case, properties) in rows {
        let router = emulator();
        let name = "cds-reused";
        create_crawler(&router, name, &properties).await;

        // While the crawler is held, its name is taken: the create that would overwrite it
        // is refused instead.
        let (status, taken) = call(
            &router,
            "CreateCrawler",
            &json!({
                "Name": name,
                "Role": REPLACEMENT_ROLE,
                "Targets": { "S3Targets": [{ "Path": REPLACEMENT_PATH }] },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a create under a held name, got {status} \
             with body {taken}"
        );
        assert_eq!(
            error_code(&taken),
            ALREADY_EXISTS,
            "case {case:?}: expected {ALREADY_EXISTS} while the crawler is held, got {taken}"
        );

        ok(&router, "DeleteCrawler", &json!({ "Name": name })).await;

        // The name is free: the same create now succeeds, and what it stores is read back.
        ok(
            &router,
            "CreateCrawler",
            &json!({
                "Name": name,
                "Role": REPLACEMENT_ROLE,
                "Targets": { "S3Targets": [{ "Path": REPLACEMENT_PATH }] },
            }),
        )
        .await;

        let replacement = get_crawler(&router, case, name).await;
        assert_eq!(
            replacement.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected the replacement crawler under {name:?}, got {replacement}"
        );
        assert_eq!(
            replacement.get("Role").and_then(Value::as_str),
            Some(REPLACEMENT_ROLE),
            "case {case:?}: expected the replacement role, got {replacement}"
        );
        assert_eq!(
            replacement
                .pointer("/Targets/S3Targets/0/Path")
                .and_then(Value::as_str),
            Some(REPLACEMENT_PATH),
            "case {case:?}: expected the replacement crawl target, got {replacement}"
        );
        assert_eq!(
            replacement.get("State").and_then(Value::as_str),
            Some(CRAWLER_STATE_READY),
            "case {case:?}: expected a fresh crawler to be {CRAWLER_STATE_READY}, got \
             {replacement}"
        );
        assert_eq!(
            replacement.get("Version").and_then(Value::as_i64),
            Some(FIRST_CRAWLER_VERSION),
            "case {case:?}: expected the first definition version {FIRST_CRAWLER_VERSION}, got \
             {replacement}"
        );
        assert_eq!(
            replacement.get("Classifiers").and_then(Value::as_array),
            Some(&vec![]),
            "case {case:?}: expected no classifiers on the replacement crawler, got \
             {replacement}"
        );
        for member in OPTIONAL_CRAWLER_MEMBERS {
            assert!(
                replacement.get(member).is_none_or(Value::is_null),
                "case {case:?}: expected no {member} on the replacement crawler, got \
                 {replacement}"
            );
        }

        // The tags were held ON the removed record, so the replacement carries none.
        let arn = format!("arn:aws:glue:us-east-1:{ACCOUNT_ID}:crawler/{name}");
        let tags = ok(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            tags.get("Tags").and_then(Value::as_object).map(|t| t.len()),
            Some(0),
            "case {case:?}: expected no tags on the replacement crawler, got {tags}"
        );

        // And the account holds ONE crawler under that name, not the removed one beside it.
        let page = ok(&router, "ListCrawlers", &json!({})).await;
        assert_eq!(
            string_list(&page, "CrawlerNames"),
            vec![name.to_string()],
            "case {case:?}: expected exactly one crawler held under {name:?}, got {page}"
        );
    }
}

// ---------------------------------------------------------------------------
// The removal reaches the crawler namespace only
// ---------------------------------------------------------------------------

/// One row of the namespace table: another resource kind holding the SAME name as the
/// crawler being removed, with the create that takes the name and the read that must still
/// resolve it afterwards.
struct Neighbour {
    /// The resource kind the row is about.
    case: &'static str,
    /// The create that takes `name` for that kind.
    create: fn(&str) -> (&'static str, Value),
    /// The read that resolves `name` for that kind, and the JSON pointer to the `Name` it
    /// reports.
    read: fn(&str) -> (&'static str, Value, &'static str),
}

fn neighbours() -> Vec<Neighbour> {
    vec![
        Neighbour {
            case: "a classifier",
            create: |name| {
                (
                    "CreateClassifier",
                    json!({ "CsvClassifier": { "Name": name, "Delimiter": ";" } }),
                )
            },
            read: |name| {
                (
                    "GetClassifier",
                    json!({ "Name": name }),
                    "/Classifier/CsvClassifier/Name",
                )
            },
        },
        Neighbour {
            case: "a job definition",
            create: |name| {
                (
                    "CreateJob",
                    json!({
                        "Name": name,
                        "Role": ROLE,
                        "Command": {
                            "Name": "glueetl",
                            "ScriptLocation": "s3://etl-scripts/etl.py",
                        },
                    }),
                )
            },
            read: |name| ("GetJob", json!({ "JobName": name }), "/Job/Name"),
        },
        Neighbour {
            case: "a workflow",
            create: |name| ("CreateWorkflow", json!({ "Name": name })),
            read: |name| ("GetWorkflow", json!({ "Name": name }), "/Workflow/Name"),
        },
        Neighbour {
            case: "a custom entity type",
            create: |name| {
                (
                    "CreateCustomEntityType",
                    json!({ "Name": name, "RegexString": "KEEP-[0-9]+" }),
                )
            },
            read: |name| ("GetCustomEntityType", json!({ "Name": name }), "/Name"),
        },
        Neighbour {
            case: "a database",
            create: |name| ("CreateDatabase", json!({ "DatabaseInput": { "Name": name } })),
            read: |name| ("GetDatabase", json!({ "Name": name }), "/Database/Name"),
        },
    ]
}

/// Removing a crawler removes the crawler: every other resource the account holds under
/// the same name is still there, and still reports that name.
///
/// The rows differ along which resource kind holds the name and along the form of the name
/// itself — the character sets and the length bound the kinds here share: `CreateWorkflow`
/// takes `OrchestrationNameString`, pattern `^[\.\-_A-Za-z0-9]+$` and length 1..128, and a
/// database name is folded to lower case, so the names below are lower-case, 1..128 long
/// and drawn from that character set
/// (`smithy_query(query="operation", operation="CreateWorkflow,CreateDatabase")`).
#[tokio::test]
async fn deleting_a_crawler_leaves_every_other_resource_held_under_that_name() {
    let names: Vec<(&str, String)> = vec![
        ("a hyphenated name", "cds-shared-name".to_string()),
        ("a single character", "x".to_string()),
        ("a name carrying each admitted punctuation", "a.b-c_1".to_string()),
        ("a name at the shared 128 ceiling", "n".repeat(128)),
    ];

    for (name_case, name) in names {
        let router = emulator();
        create_crawler(&router, &name, &json!({ "Tags": { "kind": "crawler" } })).await;
        for neighbour in neighbours() {
            let (operation, body) = (neighbour.create)(&name);
            ok(&router, operation, &body).await;
        }

        ok(&router, "DeleteCrawler", &json!({ "Name": &name })).await;

        let (status, gone) = call(&router, "GetCrawler", &json!({ "Name": &name })).await;
        assert_eq!(
            status, 400,
            "case {name_case:?}: expected HTTP 400 from GetCrawler on the removed crawler, got \
             {status} with body {gone}"
        );
        assert_eq!(
            error_code(&gone),
            ENTITY_NOT_FOUND,
            "case {name_case:?}: expected {ENTITY_NOT_FOUND} from GetCrawler, got {gone}"
        );

        for neighbour in neighbours() {
            let case = format!("{name_case} / {}", neighbour.case);
            let (operation, body, pointer) = (neighbour.read)(&name);
            let read = ok(&router, operation, &body).await;
            assert_eq!(
                read.pointer(pointer).and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: expected {operation} to still report {name:?}, got {read}"
            );
        }
    }
}
