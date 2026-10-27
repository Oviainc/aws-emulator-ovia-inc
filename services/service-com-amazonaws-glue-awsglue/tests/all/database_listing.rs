// SPDX-License-Identifier: CC0-1.0

//! `GetDatabases`: the databases one Data Catalog holds.
//!
//! RULE: `GetDatabases` "[r]etrieves all databases defined in a given Data Catalog" — so
//! it reports every database the addressed catalog actually holds, from the same records
//! `CreateDatabase` wrote and `GetDatabase` reads back, through the SAME projection the
//! single-database read serves, in folded-name order, one `MaxResults`-sized page at a
//! time with a `NextToken` that resumes the walk strictly after the last name served,
//! narrowed by `ResourceShareType` to the share asked for and by `AttributesToGet` to the
//! members named.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: which members a database was
//! created with (none, a description, a location, parameters, a target database, a
//! federated reference, a name in another case), how many databases are stored, the page
//! size asked for (both published bounds of `MaxResults` and one past each), which
//! `AttributesToGet` combination is named, and which `ResourceShareType` is asked for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, ok, ACCOUNT_ID};

/// Create one database from the `DatabaseInput` the row describes.
async fn given_database(router: &MultiServiceRouter, input: &Value) {
    ok(router, "CreateDatabase", &json!({ "DatabaseInput": input })).await;
}

/// The `Database` one `GetDatabase` reports for `name`: the single-read projection every
/// listed element is compared against.
async fn single_read(router: &MultiServiceRouter, name: &str) -> Value {
    let fetched = ok(router, "GetDatabase", &json!({ "Name": name })).await;
    fetched["Database"].clone()
}

/// The `Name` of every element of a `GetDatabases` answer, in the order it returned them.
fn listed_names(page: &Value) -> Vec<String> {
    page["DatabaseList"]
        .as_array()
        .expect("DatabaseList is a list")
        .iter()
        .map(|database| {
            database["Name"]
                .as_str()
                .expect("Database.Name is a string")
                .to_string()
        })
        .collect()
}

/// Walk every page `GetDatabases` hands out for `request`, following its own `NextToken`,
/// and return the elements in the order they were listed.
async fn walk_listing(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(cursor) = &token {
            page_request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetDatabases", &page_request).await;
        let elements = page["DatabaseList"]
            .as_array()
            .expect("DatabaseList is a list")
            .clone();
        if let Some(size) = request.get("MaxResults").and_then(Value::as_u64) {
            assert!(
                elements.len() as u64 <= size,
                "case {case:?}: a page must not exceed MaxResults {size}, got {elements:?}"
            );
        }
        listed.extend(elements);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= 64,
            "case {case:?}: the walk must terminate, listed {listed:?} so far"
        );
    }
    listed
}

/// The member names an element carries, sorted, so an assertion names exactly what was
/// returned rather than only what was expected.
fn member_names(element: &Value) -> Vec<String> {
    let mut members: Vec<String> = element
        .as_object()
        .expect("a Database element is a structure")
        .keys()
        .cloned()
        .collect();
    members.sort();
    members
}

/// The `DatabaseInput`s the projection rows are built from: every member a client can set
/// on a database, set and left unset, so an element that drops one or fills one in is
/// caught. Source: `smithy_query shape=DatabaseInput` on this crate's model.
fn projection_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("nothing but a name", json!({ "Name": "bare_db" })),
        (
            "a description and no location",
            json!({
                "Name": "described_db",
                "Description": "the catalog of a warehouse",
            }),
        ),
        (
            "a location and no description",
            json!({
                "Name": "located_db",
                "LocationUri": "s3://bucket/located_db/",
            }),
        ),
        (
            "a description, a location and parameters",
            json!({
                "Name": "rich_db",
                "Description": "everything a client can set",
                "LocationUri": "hdfs://namenode:8020/rich_db",
                "Parameters": { "owner": "data-eng", "classification": "internal" },
            }),
        ),
        (
            "a resource link to another database",
            json!({
                "Name": "linked_db",
                "TargetDatabase": {
                    "CatalogId": ACCOUNT_ID,
                    "DatabaseName": "bare_db",
                    "Region": "us-east-1",
                },
            }),
        ),
        (
            "a federated reference",
            json!({
                "Name": "federated_db",
                "FederatedDatabase": {
                    "Identifier": "arn:aws:glue:us-east-1:000000000000:database/external",
                    "ConnectionName": "hive-metastore",
                },
            }),
        ),
        (
            // `DatabaseInput$Name` is "folded to lowercase when it is stored" for Hive
            // compatibility, so the listing reports the folded spelling.
            "a name in another case",
            json!({
                "Name": "Mixed_Case_DB",
                "Description": "folded for Hive compatibility",
            }),
        ),
        (
            "an empty description, which is inside the modelled length",
            json!({ "Name": "empty_description_db", "Description": "" }),
        ),
    ]
}

#[tokio::test]
async fn every_listed_database_is_the_single_read_projection() {
    let router = emulator();
    let rows = projection_rows();
    for (_, input) in &rows {
        given_database(&router, input).await;
    }

    let listed = walk_listing(&router, "the whole catalog", &json!({})).await;

    for (case, input) in &rows {
        let name = input["Name"].as_str().expect("a database name");
        let expected = single_read(&router, name).await;
        let folded = name.to_lowercase();
        let element = listed
            .iter()
            .find(|element| element["Name"] == json!(folded))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {folded} must appear in {:?}", listed_names(&json!({ "DatabaseList": listed.clone() })))
            });
        assert_eq!(
            element, &expected,
            "case {case:?}: a listed element must be the GetDatabase projection, got \
             {element:?} against {expected:?}"
        );
        // A member the create set round-trips; one it never set stays absent, rather than
        // being filled in with a value the client never sent.
        for member in ["Description", "LocationUri", "Parameters"] {
            let sent = input.get(member).cloned().unwrap_or(Value::Null);
            let got = element.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                got, sent,
                "case {case:?}: {member} must round-trip on the listed element; expected \
                 {sent:?}, got {got:?}"
            );
        }
    }

    assert_eq!(
        listed.len(),
        rows.len(),
        "every stored database must be listed, got {:?}",
        listed_names(&json!({ "DatabaseList": listed.clone() }))
    );
}

#[tokio::test]
async fn every_stored_database_is_listed_in_name_order_and_paged() {
    // (case, how many databases are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no databases at all", 0, None),
        ("one database, no page size", 1, None),
        // The published lower bound of `MaxResults` (`CatalogGetterPageSize`, range 1..100).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page exactly the stored count", 4, Some(4)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(100)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored).map(|index| format!("db_{index:02}")).collect();
        for name in expected.iter().rev() {
            given_database(&router, &json!({ "Name": name })).await;
        }

        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        let listed: Vec<String> = walk_listing(&router, case, &request)
            .await
            .iter()
            .map(|element| {
                element["Name"]
                    .as_str()
                    .expect("Database.Name is a string")
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: every stored database must be listed in folded-name order, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_last_page_of_a_listing_carries_no_next_token() {
    let router = emulator();
    for index in 0..3 {
        given_database(&router, &json!({ "Name": format!("db_{index:02}") })).await;
    }

    // (case, the page size, whether that page is the last one)
    let rows: &[(&str, i32, bool)] = &[
        ("a page that stops short of the end continues", 1, false),
        ("a page that stops one short of the end continues", 2, false),
        ("a page that reaches the end stops", 3, true),
        ("a page past the end stops", 10, true),
    ];
    for (case, size, is_last) in rows {
        let page = ok(&router, "GetDatabases", &json!({ "MaxResults": size })).await;
        let token = page.get("NextToken").and_then(Value::as_str);
        assert_eq!(
            token.is_none(),
            *is_last,
            "case {case:?}: expected last page {is_last}, got token {token:?} in {page}"
        );
    }
}

#[tokio::test]
async fn attributes_to_get_narrows_each_element_to_the_members_it_names() {
    let router = emulator();
    given_database(&router, &json!({ "Name": "plain_db", "Description": "no link" })).await;
    given_database(
        &router,
        &json!({
            "Name": "linked_db",
            "TargetDatabase": { "CatalogId": ACCOUNT_ID, "DatabaseName": "plain_db" },
        }),
    )
    .await;

    // (case, AttributesToGet, the members a linked element carries, the members a plain
    // element carries). Source: `DatabaseAttributes`, whose values are exactly `NAME` and
    // `TARGET_DATABASE`; the request "must include the NAME".
    let rows: Vec<(&str, Value, Vec<&str>, Vec<&str>)> = vec![
        ("NAME alone", json!(["NAME"]), vec!["Name"], vec!["Name"]),
        (
            "NAME and TARGET_DATABASE",
            json!(["NAME", "TARGET_DATABASE"]),
            vec!["Name", "TargetDatabase"],
            vec!["Name"],
        ),
        (
            "the same two in the other order",
            json!(["TARGET_DATABASE", "NAME"]),
            vec!["Name", "TargetDatabase"],
            vec!["Name"],
        ),
    ];

    for (case, attributes, linked_members, plain_members) in &rows {
        let page = ok(
            &router,
            "GetDatabases",
            &json!({ "AttributesToGet": attributes }),
        )
        .await;
        let elements = page["DatabaseList"]
            .as_array()
            .expect("DatabaseList is a list")
            .clone();
        assert_eq!(
            listed_names(&page),
            vec!["linked_db".to_string(), "plain_db".to_string()],
            "case {case:?}: a projection narrows each element, not the listing, got {elements:?}"
        );
        for (element, expected) in elements.iter().zip([linked_members, plain_members]) {
            let expected: Vec<String> = {
                let mut names: Vec<String> =
                    expected.iter().map(|name| name.to_string()).collect();
                names.sort();
                names
            };
            assert_eq!(
                member_names(element),
                expected,
                "case {case:?}: expected members {expected:?}, got {element:?}"
            );
        }
    }

    // An `AttributesToGet` the model refuses: the empty list it "doesn't accept", and any
    // combination that does not include `NAME`.
    let refused: Vec<(&str, Value)> = vec![
        ("the empty list", json!([])),
        ("a combination without NAME", json!(["TARGET_DATABASE"])),
    ];
    for (case, attributes) in &refused {
        let (status, body) = call(
            &router,
            "GetDatabases",
            &json!({ "AttributesToGet": attributes }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: AttributesToGet {attributes} must be refused, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn resource_share_type_selects_which_share_is_listed() {
    let router = emulator();
    given_database(&router, &json!({ "Name": "local_db" })).await;
    given_database(
        &router,
        &json!({
            "Name": "federated_db",
            "FederatedDatabase": {
                "Identifier": "arn:aws:glue:us-east-1:000000000000:database/external",
                "ConnectionName": "hive-metastore",
            },
        }),
    )
    .await;
    given_database(
        &router,
        &json!({
            "Name": "another_federated_db",
            "FederatedDatabase": { "Identifier": "hive://metastore/sales" },
        }),
    )
    .await;

    // (case, ResourceShareType, the names expected). A stored database is federated
    // exactly when it carries a `FederatedDatabase`; none is foreign, because this store
    // holds one account's catalog and nothing in it was shared from another account.
    let rows: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        (
            "no share type named",
            None,
            vec!["another_federated_db", "federated_db", "local_db"],
        ),
        (
            "ALL",
            Some("ALL"),
            vec!["another_federated_db", "federated_db", "local_db"],
        ),
        (
            "FEDERATED",
            Some("FEDERATED"),
            vec!["another_federated_db", "federated_db"],
        ),
        ("FOREIGN", Some("FOREIGN"), vec![]),
    ];

    for (case, share, expected) in &rows {
        let mut request = json!({});
        if let Some(share) = share {
            request["ResourceShareType"] = json!(share);
        }
        let listed: Vec<String> = walk_listing(&router, case, &request)
            .await
            .iter()
            .map(|element| {
                element["Name"]
                    .as_str()
                    .expect("Database.Name is a string")
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // A value the enum does not declare is refused rather than quietly listing
    // everything.
    let (status, body) = call(
        &router,
        "GetDatabases",
        &json!({ "ResourceShareType": "LOCAL" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a ResourceShareType the enum does not declare must be refused, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_catalog_with_no_databases_lists_none_and_another_accounts_is_refused() {
    let router = emulator();

    // This account's catalog, before anything wrote a database into it: an empty listing,
    // not a miss.
    let page = ok(&router, "GetDatabases", &json!({})).await;
    assert_eq!(
        page["DatabaseList"],
        json!([]),
        "a catalog holding no databases lists none, got {page}"
    );
    assert!(
        page.get("NextToken").and_then(Value::as_str).is_none(),
        "an empty listing is the last page, got {page}"
    );

    // The same request against the catalog of another account, which is reachable only
    // through that catalog's own resource policy.
    let (status, body) = call(
        &router,
        "GetDatabases",
        &json!({ "CatalogId": "210987654321" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a listing of another account's catalog must be refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("AccessDeniedException"),
        "expected AccessDeniedException, got {} in {body}",
        error_code(&body)
    );
}

#[tokio::test]
async fn max_results_is_served_at_its_published_bounds_and_refused_past_them() {
    let router = emulator();
    given_database(&router, &json!({ "Name": "bounded_db" })).await;

    // `MaxResults` targets `CatalogGetterPageSize`, range 1..100.
    for accepted in [1, 100] {
        let (status, body) = call(
            &router,
            "GetDatabases",
            &json!({ "MaxResults": accepted }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults {accepted}: a published bound must be served, got {status} with \
             {body}"
        );
    }
    for refused in [0, 101] {
        let (status, body) = call(&router, "GetDatabases", &json!({ "MaxResults": refused })).await;
        assert_eq!(
            status, 400,
            "case MaxResults {refused}: a value outside the modelled range must be refused, \
             got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_continuation_token_resumes_strictly_after_the_name_it_names() {
    let router = emulator();
    let names: Vec<String> = (0..6).map(|index| format!("page_db_{index}")).collect();
    for name in &names {
        given_database(&router, &json!({ "Name": name })).await;
    }

    // Each row walks the same listing at a different page size and asserts the pages
    // concatenate to the whole listing exactly once, with no name served twice.
    for page_size in [1_i32, 2, 3, 4, 6, 7] {
        let case = format!("MaxResults {page_size}");
        let listed: Vec<String> = walk_listing(&router, &case, &json!({ "MaxResults": page_size }))
            .await
            .iter()
            .map(|element| {
                element["Name"]
                    .as_str()
                    .expect("Database.Name is a string")
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed, names,
            "case {case:?}: the pages must concatenate to the whole listing, got {listed:?}"
        );
    }

    // A token naming a database in the middle resumes at the first name strictly after it,
    // whatever the stored set does between calls — including after that database is gone.
    let page = ok(
        &router,
        "GetDatabases",
        &json!({ "MaxResults": 2, "NextToken": "page_db_2" }),
    )
    .await;
    assert_eq!(
        listed_names(&page),
        vec!["page_db_3".to_string(), "page_db_4".to_string()],
        "a token resumes strictly after the name it carries, got {page}"
    );
    ok(&router, "DeleteDatabase", &json!({ "Name": "page_db_3" })).await;
    let page = ok(
        &router,
        "GetDatabases",
        &json!({ "MaxResults": 2, "NextToken": "page_db_2" }),
    )
    .await;
    assert_eq!(
        listed_names(&page),
        vec!["page_db_4".to_string(), "page_db_5".to_string()],
        "a token still resumes after the name it carries once that database is deleted, got \
         {page}"
    );
}
