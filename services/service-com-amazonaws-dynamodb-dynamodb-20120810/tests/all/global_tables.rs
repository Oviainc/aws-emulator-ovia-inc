// SPDX-License-Identifier: CC0-1.0

//! What `ListGlobalTables` reports, and what it does not.
//!
//! The rule these tests pin:
//!
//! > a page of `ListGlobalTables` holds the global tables this store holds whose
//! > replication group has a replica in the requested Region — in ascending name order,
//! > resumed after `ExclusiveStartGlobalTableName`, truncated at `Limit` (100 when the
//! > request names none) with the name it stopped on reported in
//! > `LastEvaluatedGlobalTableName` — and a table that belongs to no replication group
//! > is not a global table, so no page reports it.
//!
//! A replication group spans Regions, so it is recorded in the store as the relation
//! between a global table's name and the Regions holding its replicas
//! (`ComAmazonawsDynamodbDynamodb20120810State::global_table_groups`). The surface that
//! ESTABLISHES one — `UpdateTable`'s `ReplicaUpdates` — is still scaffolded, so the
//! groups a reporting test needs are written straight into that store, exactly as the
//! replica surface will write them; everything the operation is asked about then goes
//! over the wire through the same router the emulator's front doors use.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::ComAmazonawsDynamodbDynamodb20120810State;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, call_in, emulator, emulator_and_engine, error_code, message, ok};

/// Record one replication group in the store: the global table's name and the Regions
/// holding a replica of it.
fn seed_group(engine: &Arc<engine::InMemoryEngine>, name: &str, regions: &[&str]) {
    engine
        .state::<ComAmazonawsDynamodbDynamodb20120810State>()
        .lock()
        .global_table_groups
        .insert(
            name.to_string(),
            regions.iter().map(ToString::to_string).collect::<BTreeSet<_>>(),
        );
}

/// The `GlobalTableName`s one page reports, in the order the page carries them.
fn listed_names(page: &Value) -> Vec<String> {
    page["GlobalTables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|table| {
            table["GlobalTableName"]
                .as_str()
                .map(ToString::to_string)
        })
        .collect()
}

/// The `ReplicationGroup` Regions one page reports for `name`.
fn listed_regions(page: &Value, name: &str) -> Vec<String> {
    page["GlobalTables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|table| table["GlobalTableName"] == json!(name))
        .map(|table| {
            table["ReplicationGroup"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|replica| replica["RegionName"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The simplest `CreateTable` body there is, under `name`.
fn create_table_body(name: &str) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

/// Every global table name the store holds, read page by page with `limit`-sized pages:
/// each page is resumed after the previous page's `LastEvaluatedGlobalTableName`, which
/// is what a client paging this operation does.
async fn walk_pages(router: &MultiServiceRouter, limit: i64) -> Vec<String> {
    let mut names = Vec::new();
    let mut start: Option<String> = None;
    loop {
        let mut request = json!({"Limit": limit});
        if let Some(start) = start.as_deref() {
            request["ExclusiveStartGlobalTableName"] = json!(start);
        }
        let page = ok(router, "ListGlobalTables", request).await;
        let page_names = listed_names(&page);
        assert!(
            page_names.len() <= usize::try_from(limit).unwrap_or(0),
            "case walk with Limit {limit}: a page holds at most {limit} names, got {page}"
        );
        names.extend(page_names);
        match page["LastEvaluatedGlobalTableName"].as_str() {
            Some(last) => start = Some(last.to_string()),
            None => break,
        }
        assert!(
            names.len() <= 64,
            "case walk with Limit {limit}: the walk must terminate, got {names:?}"
        );
    }
    names
}

#[tokio::test]
async fn a_table_that_belongs_to_no_replication_group_is_not_a_global_table() {
    let router = emulator();
    // One row per shape a client's own tables can take: a single table, several tables
    // in one Region, and one name created in TWO Regions — which is two independent
    // tables that happen to share a name, not a replication group. None of them is a
    // global table, whichever Region the listing asks about.
    let rows: [(&str, &[(&str, &str)]); 3] = [
        ("one table", &[("Solo", "us-east-1")]),
        (
            "three tables in one Region",
            &[
                ("Alpha", "us-east-1"),
                ("Bravo", "us-east-1"),
                ("Charlie", "us-east-1"),
            ],
        ),
        (
            "the same name in two Regions",
            &[("Shared", "us-east-1"), ("Shared", "us-west-2")],
        ),
    ];

    for (case, tables) in rows {
        for (name, region) in tables {
            let created = call_in(&router, region, "CreateTable", create_table_body(name)).await;
            assert_eq!(
                created.0, 200,
                "case {case:?}: expected {name} to be created in {region}, got {:?}",
                created.1
            );
        }

        for asked_about in [None, Some("us-east-1"), Some("us-west-2")] {
            let mut request = json!({});
            if let Some(region) = asked_about {
                request["RegionName"] = json!(region);
            }
            let page = ok(&router, "ListGlobalTables", request).await;
            assert_eq!(
                listed_names(&page),
                Vec::<String>::new(),
                "case {case:?} asked about {asked_about:?}: a table in no replication group \
                 is not a global table, got {page}"
            );
            assert_eq!(
                page["LastEvaluatedGlobalTableName"],
                Value::Null,
                "case {case:?} asked about {asked_about:?}: an exhausted page names no \
                 resume point, got {page}"
            );
        }
    }

    // ...and the tables themselves are there to be listed, in the Region each was
    // created in.
    let tables = ok(&router, "ListTables", json!({})).await;
    for name in ["Solo", "Alpha", "Bravo", "Charlie", "Shared"] {
        assert!(
            tables["TableNames"]
                .as_array()
                .is_some_and(|names| names.contains(&json!(name))),
            "case the store's own tables: expected {name} among them, got {tables}"
        );
    }
}

#[tokio::test]
async fn a_page_reports_the_groups_with_a_replica_in_the_region_it_asks_about() {
    let (router, engine) = emulator_and_engine();
    // Three groups differing in how many Regions they span and in which: two that
    // overlap in one Region, and one that overlaps neither.
    let groups: [(&str, &[&str]); 3] = [
        ("Atlas.Orders", &["us-east-1", "us-west-2"]),
        ("Borealis_Users", &["eu-central-1", "eu-west-1", "us-east-1"]),
        ("cedar-events", &["ap-south-1"]),
    ];
    for (name, regions) in groups {
        seed_group(&engine, name, regions);
    }
    // A table of its own in one of those Regions, to show the page is the groups and
    // not the tables.
    ok(&router, "CreateTable", create_table_body("Plain")).await;

    // One row per Region a request can ask about, plus the request that asks about
    // none: the expected names are those whose group holds a replica there.
    let rows: [(&str, Option<&str>, &[&str]); 6] = [
        (
            "no Region at all",
            None,
            &["Atlas.Orders", "Borealis_Users", "cedar-events"],
        ),
        (
            "a Region two groups share",
            Some("us-east-1"),
            &["Atlas.Orders", "Borealis_Users"],
        ),
        ("a Region one group holds", Some("us-west-2"), &["Atlas.Orders"]),
        (
            "another Region the same one group holds",
            Some("eu-west-1"),
            &["Borealis_Users"],
        ),
        (
            "the Region of the single-Region group",
            Some("ap-south-1"),
            &["cedar-events"],
        ),
        ("a Region no group holds", Some("sa-east-1"), &[]),
    ];

    for (case, asked_about, expected) in rows {
        let mut request = json!({});
        if let Some(region) = asked_about {
            request["RegionName"] = json!(region);
        }
        let page = ok(&router, "ListGlobalTables", request).await;
        let actual = listed_names(&page);
        assert_eq!(
            actual,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?} ({asked_about:?}): expected exactly the groups with a replica \
             there, in ascending name order, got {actual:?} from {page}"
        );
        // Every entry carries the Regions of its own group, whichever Region was asked
        // about: the group is the resource, and the question only selects it.
        for name in expected {
            let regions = listed_regions(&page, name);
            let seeded: Vec<String> = groups
                .iter()
                .find(|(group, _)| group == name)
                .map(|(_, regions)| {
                    let mut regions: Vec<String> =
                        regions.iter().map(ToString::to_string).collect();
                    regions.sort();
                    regions
                })
                .unwrap_or_default();
            assert_eq!(
                regions, seeded,
                "case {case:?} ({asked_about:?}): expected {name}'s replication group to \
                 be {seeded:?}, got {regions:?} from {page}"
            );
        }
    }
}

/// One row of the page-size table: what the case is, the `Limit` the request names (if
/// any), the names the page must hold and the resume point it must report.
type LimitRow = (
    &'static str,
    Option<i64>,
    &'static [&'static str],
    Option<&'static str>,
);

#[tokio::test]
async fn limit_truncates_the_page_and_names_where_the_next_one_resumes() {
    let (router, engine) = emulator_and_engine();
    // Five groups in one Region, so `Limit` is the only thing that decides a page.
    let names = ["g1.one", "g2-two", "g3_three", "g4four", "g5.five"];
    for name in names {
        seed_group(&engine, name, &["us-east-1"]);
    }

    // One row per interesting page size: the modelled minimum, a size inside the set,
    // the size of the set exactly, one past it, and the request that names no size at
    // all (which takes the documented default of 100 and so holds everything).
    let rows: [LimitRow; 6] = [
        ("the modelled minimum", Some(1), &["g1.one"], Some("g1.one")),
        (
            "a size inside the set",
            Some(2),
            &["g1.one", "g2-two"],
            Some("g2-two"),
        ),
        (
            "one below the number of groups",
            Some(4),
            &["g1.one", "g2-two", "g3_three", "g4four"],
            Some("g4four"),
        ),
        (
            "exactly the number of groups",
            Some(5),
            &["g1.one", "g2-two", "g3_three", "g4four", "g5.five"],
            None,
        ),
        (
            "one past the number of groups",
            Some(6),
            &["g1.one", "g2-two", "g3_three", "g4four", "g5.five"],
            None,
        ),
        (
            "no size at all",
            None,
            &["g1.one", "g2-two", "g3_three", "g4four", "g5.five"],
            None,
        ),
    ];

    for (case, limit, expected, resume) in rows {
        let mut request = json!({});
        if let Some(limit) = limit {
            request["Limit"] = json!(limit);
        }
        let page = ok(&router, "ListGlobalTables", request).await;
        let actual = listed_names(&page);
        assert_eq!(
            actual,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?} (Limit {limit:?}): expected the first {} names, got {actual:?} \
             from {page}",
            expected.len()
        );
        assert_eq!(
            page["LastEvaluatedGlobalTableName"],
            resume.map_or(Value::Null, |name| json!(name)),
            "case {case:?} (Limit {limit:?}): expected the resume point {resume:?}, got {page}"
        );
    }

    // Paging with any of those sizes reads every group exactly once, in ascending name
    // order — the resume point is where the next page starts, not a name it repeats.
    for limit in [1, 2, 3, 5, 6] {
        let walked = walk_pages(&router, limit).await;
        assert_eq!(
            walked,
            names.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case walk with Limit {limit}: expected every group once in name order, got \
             {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_after_the_name_the_request_names() {
    let (router, engine) = emulator_and_engine();
    let names = ["aardvark.gt", "marmot.gt", "zebu.gt"];
    for name in names {
        seed_group(&engine, name, &["us-east-1"]);
    }

    // One row per place a resume point can fall: before every name, on a name,
    // between two names, on the last name, and after every name. "The first global
    // table name that this operation will evaluate" is the one AFTER it.
    let rows: [(&str, &str, &[&str]); 5] = [
        (
            "before every name",
            "aaa",
            &["aardvark.gt", "marmot.gt", "zebu.gt"],
        ),
        ("on the first name", "aardvark.gt", &["marmot.gt", "zebu.gt"]),
        ("between two names", "n.gt", &["zebu.gt"]),
        ("on the last name", "zebu.gt", &[]),
        ("after every name", "zzz", &[]),
    ];

    for (case, start, expected) in rows {
        let page = ok(
            &router,
            "ListGlobalTables",
            json!({"ExclusiveStartGlobalTableName": start}),
        )
        .await;
        let actual = listed_names(&page);
        assert_eq!(
            actual,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?} (resuming after {start:?}): expected the names after it, got \
             {actual:?} from {page}"
        );
    }
}

#[tokio::test]
async fn a_limit_below_its_modelled_minimum_is_refused() {
    let router = emulator();
    // `ListGlobalTables$Limit` is a `PositiveIntegerObject` with `@range(min: 1)`, so 1
    // is served and anything below it is refused with the validator's own sentence.
    let served = ok(&router, "ListGlobalTables", json!({"Limit": 1})).await;
    assert_eq!(
        listed_names(&served),
        Vec::<String>::new(),
        "case the bound itself (Limit 1): expected an empty page rather than a refusal, \
         got {served}"
    );

    for (case, limit) in [("one below the bound", 0), ("far below it", -7)] {
        let (status, body) = call(&router, "ListGlobalTables", json!({"Limit": limit})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} (Limit {limit}): expected 400 ValidationException, got {status} \
             with {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Value '{limit}' at 'limit' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1"
            ),
            "case {case:?} (Limit {limit}): expected the validator's sentence, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn listing_global_tables_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator_and_engine();
    for (name, regions) in [
        ("kept.one", &["us-east-1", "us-west-2"][..]),
        ("kept.two", &["eu-west-1"][..]),
    ] {
        seed_group(&engine, name, regions);
    }
    ok(&router, "CreateTable", create_table_body("Untouched")).await;

    let before = ok(&router, "ListGlobalTables", json!({})).await;
    // Read it every way the operation can be read: narrowed, truncated and resumed.
    for request in [
        json!({"RegionName": "us-east-1"}),
        json!({"Limit": 1}),
        json!({"ExclusiveStartGlobalTableName": "kept.one"}),
        json!({"RegionName": "eu-west-1", "Limit": 1}),
    ] {
        ok(&router, "ListGlobalTables", request).await;
    }
    let after = ok(&router, "ListGlobalTables", json!({})).await;
    assert_eq!(
        after, before,
        "case repeated reads: a report must not change what a later caller sees, got \
         {after} after {before}"
    );

    // ...and the store still holds both groups and the table beside them.
    let groups: Vec<String> = engine
        .state::<ComAmazonawsDynamodbDynamodb20120810State>()
        .lock()
        .global_table_groups
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        groups,
        vec!["kept.one".to_string(), "kept.two".to_string()],
        "case repeated reads: expected the stored groups untouched, got {groups:?}"
    );
    let described = ok(&router, "DescribeTable", json!({"TableName": "Untouched"})).await;
    assert_eq!(
        described["Table"]["TableName"],
        json!("Untouched"),
        "case repeated reads: expected the table beside the groups untouched, got {described}"
    );
}
