// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetTableVersions`: reading the WHOLE version history of one
//! table out of the catalog.
//!
//! The rule under test: a version listing reports every version the catalog holds of the
//! table it addresses — the live definition, which is the version the table's `VersionId`
//! names, and every earlier definition an update archived — each as the same
//! `TableVersion` the single-version read serves for it, in the order the counters were
//! minted, paged by `MaxResults` with a `NextToken` that resumes immediately after the
//! last version served.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness, dispatched by the same `MultiServiceRouter` the production
//! front door uses.
//!
//! The suites are tables: a row is one history (how many writes the table has had and
//! which member set each one submitted), one page size, or one unlistable request. The
//! recorded cases — a freshly created table whose single listed version matches
//! `GetTable`, and a history read back around a version deletion — are rows of the first
//! two suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::table_versions::{get_table, get_table_version, histories, write_history, TABLE};
use crate::wire::{call, emulator, error_code, ok};

/// The largest page `GetTableVersions` serves, which is also the page it serves when the
/// request names no `MaxResults`. Source: the model's `MaxResults` member, whose target
/// `CatalogGetterPageSize` has range 1..100 (`smithy_query operation=GetTableVersions`).
const MAX_PAGE_SIZE: usize = 100;

/// The `VersionId`s a listing page reports, in the order the service returned them.
fn version_ids(page: &Value) -> Vec<String> {
    page["TableVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("TableVersions must be a list, got {page}"))
        .iter()
        .map(|version| {
            version["VersionId"]
                .as_str()
                .unwrap_or_else(|| panic!("TableVersion.VersionId must be a string, got {version}"))
                .to_string()
        })
        .collect()
}

/// One `GetTableVersions` call: the table's versions, optionally bounded by `page_size`
/// and resumed from `token`.
async fn list_versions(
    router: &MultiServiceRouter,
    database: &str,
    page_size: Option<i64>,
    token: Option<&str>,
) -> Value {
    let mut request = json!({ "DatabaseName": database, "TableName": TABLE });
    if let Some(size) = page_size {
        request["MaxResults"] = json!(size);
    }
    if let Some(cursor) = token {
        request["NextToken"] = json!(cursor);
    }
    ok(router, "GetTableVersions", &request).await
}

/// Walk the whole listing at `page_size`, following each `NextToken`, and return one entry
/// per page: the version ids it carried and whether it came with a continuation token.
async fn walk_pages(
    router: &MultiServiceRouter,
    database: &str,
    page_size: Option<i64>,
) -> Vec<(Vec<String>, bool)> {
    let mut pages: Vec<(Vec<String>, bool)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let answer = list_versions(router, database, page_size, token.as_deref()).await;
        token = answer["NextToken"].as_str().map(str::to_string);
        pages.push((version_ids(&answer), token.is_some()));
        if token.is_none() {
            return pages;
        }
        assert!(
            pages.len() < 1_000,
            "case page size {page_size:?}: a listing must terminate, got {pages:?}"
        );
    }
}

/// The version ids a table with `writes` writes has, in the order the counters were
/// minted: one version per write, numbered from the first.
fn expected_ids(writes: usize) -> Vec<String> {
    (0..writes).map(|counter| counter.to_string()).collect()
}

/// Every version of a table is listed, under the counter that names it and in the order
/// the counters were minted, and each element is exactly what the single-version read
/// serves for that version.
#[tokio::test]
async fn every_version_of_a_table_is_listed() {
    for (case, database, history) in histories() {
        let router = emulator();
        write_history(&router, database, &history).await;

        let page = list_versions(&router, database, None, None).await;
        assert!(
            page["NextToken"].is_null(),
            "case {case}: a history shorter than one page is served whole, got {page}"
        );
        assert_eq!(
            version_ids(&page),
            expected_ids(history.len()),
            "case {case}: every version of the table is listed in counter order, got {page}"
        );

        for version in page["TableVersions"]
            .as_array()
            .expect("TableVersions is a list")
        {
            let version_id = version["VersionId"].as_str().expect("a version id");
            let read = get_table_version(&router, database, Some(version_id)).await;
            assert_eq!(
                version, &read,
                "case {case} at version {version_id}: the listing serves what the \
                 single-version read serves; expected {read}, got {version}"
            );
        }

        // The newest listed version is the table itself: a table that has never been
        // updated therefore has exactly one version, and it matches `GetTable`.
        let live = get_table(&router, database).await;
        let newest = page["TableVersions"]
            .as_array()
            .and_then(|versions| versions.last())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            newest["VersionId"], live["VersionId"],
            "case {case}: the newest listed version is the live one {}, got {newest}",
            live["VersionId"]
        );
        assert_eq!(
            newest["Table"], live,
            "case {case}: the newest listed version is the live table {live}, got {newest}"
        );
    }
}

/// The history these paging rows walk: six writes, each submitting a different member set,
/// so a page boundary can fall anywhere in a history whose definitions differ.
fn paging_history() -> Vec<Value> {
    vec![
        json!({ "Name": TABLE }),
        json!({ "Name": TABLE, "Description": "the second definition" }),
        json!({ "Name": TABLE, "Owner": "analytics", "Retention": 7 }),
        json!({ "Name": TABLE, "Parameters": { "classification": "json" } }),
        json!({
            "Name": TABLE,
            "StorageDescriptor": {
                "Columns": [ { "Name": "id", "Type": "string" } ],
                "Location": "s3://bucket/fifth",
            },
        }),
        json!({ "Name": TABLE, "Description": "the sixth definition", "TableType": "EXTERNAL_TABLE" }),
    ]
}

/// A page carries at most `MaxResults` versions and a token exactly while versions remain,
/// and following that token resumes immediately after the last version served — so the
/// pages concatenated are the whole history, once each, whatever the page size.
#[tokio::test]
async fn a_listing_pages_over_the_whole_history() {
    let history = paging_history();
    let expected = expected_ids(history.len());

    // One row per page size: below the history, at it, above it, the published upper bound
    // of `MaxResults`, and the request that names no page size at all.
    for page_size in [Some(1), Some(2), Some(3), Some(5), Some(6), Some(7), Some(100), None] {
        let router = emulator();
        write_history(&router, "paged_db", &history).await;
        let pages = walk_pages(&router, "paged_db", page_size).await;

        let bound = page_size.unwrap_or(MAX_PAGE_SIZE as i64) as usize;
        let served: Vec<String> = pages
            .iter()
            .flat_map(|(ids, _)| ids.iter().cloned())
            .collect();
        assert_eq!(
            served, expected,
            "case page size {page_size:?}: the pages concatenated are the whole history once \
             each, got {pages:?}"
        );
        for (index, (ids, has_token)) in pages.iter().enumerate() {
            assert!(
                ids.len() <= bound,
                "case page size {page_size:?}: page {index} must carry at most {bound} \
                 versions, got {ids:?}"
            );
            let last_page = index + 1 == pages.len();
            assert_eq!(
                *has_token, !last_page,
                "case page size {page_size:?}: page {index} carries a continuation token \
                 exactly while versions remain, got {pages:?}"
            );
            if !last_page {
                assert_eq!(
                    ids.len(),
                    bound,
                    "case page size {page_size:?}: a page followed by another is full, got \
                     {ids:?}"
                );
            }
            assert!(
                !ids.is_empty(),
                "case page size {page_size:?}: page {index} of a non-empty history must carry \
                 a version, got {pages:?}"
            );
        }
    }
}

/// A history longer than one page is served at the model's largest page size when the
/// request names none, and the token resumes the remainder.
#[tokio::test]
async fn a_history_longer_than_one_page_is_bounded_by_the_model() {
    let router = emulator();
    // One write past the page bound, so the first page is full and one version is left
    // over.
    let history: Vec<Value> = (0..=MAX_PAGE_SIZE)
        .map(|counter| json!({ "Name": TABLE, "Description": format!("definition {counter}") }))
        .collect();
    write_history(&router, "long_db", &history).await;

    let first = list_versions(&router, "long_db", None, None).await;
    assert_eq!(
        version_ids(&first).len(),
        MAX_PAGE_SIZE,
        "case a history of {} versions: the default page is the largest the model accepts, got \
         {} versions",
        history.len(),
        version_ids(&first).len()
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case a history past one page: expected a token, got {first}"))
        .to_string();

    let second = list_versions(&router, "long_db", None, Some(&token)).await;
    assert_eq!(
        version_ids(&second),
        vec![MAX_PAGE_SIZE.to_string()],
        "case the remainder of a history past one page: expected the one version left, got \
         {second}"
    );
    assert!(
        second["NextToken"].is_null(),
        "case the remainder of a history past one page: nothing remains, got {second}"
    );
}

/// `MaxResults` one past either published bound of `CatalogGetterPageSize` (range 1..100)
/// is refused rather than served.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = emulator();
    write_history(
        &router,
        "bounded_db",
        &[json!({ "Name": TABLE, "Description": "as created" })],
    )
    .await;

    for (case, max_results) in [
        ("MaxResults one below the range", 0),
        ("MaxResults one above the range", MAX_PAGE_SIZE as i64 + 1),
    ] {
        let (status, body) = call(
            &router,
            "GetTableVersions",
            &json!({
                "DatabaseName": "bounded_db",
                "TableName": TABLE,
                "MaxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a page size outside the modelled range must be refused, got {status} \
             with {body}"
        );
    }

    // Each published bound of the range is served.
    for max_results in [1, MAX_PAGE_SIZE as i64] {
        let page = list_versions(&router, "bounded_db", Some(max_results), None).await;
        assert_eq!(
            version_ids(&page),
            vec!["0".to_string()],
            "case MaxResults {max_results}: a bound of the range is served, got {page}"
        );
    }
}

/// Only the versions the catalog HOLDS are listed: an update that asks not to archive the
/// definition it replaces leaves that version out of the history, so the counters a
/// listing reports can have gaps.
#[tokio::test]
async fn only_the_versions_the_catalog_holds_are_listed() {
    // (case, the SkipArchive member of the update, the version ids left afterwards)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an update that does not mention SkipArchive",
            Value::Null,
            vec!["0", "1"],
        ),
        ("an update with SkipArchive false", json!(false), vec!["0", "1"]),
        ("an update with SkipArchive true", json!(true), vec!["1"]),
    ];

    for (case, skip_archive, expected) in cases {
        let router = emulator();
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": "archive_db" } }),
        )
        .await;
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": "archive_db",
                "TableInput": { "Name": TABLE, "Description": "as created" },
            }),
        )
        .await;
        let mut update = json!({
            "DatabaseName": "archive_db",
            "TableInput": { "Name": TABLE, "Description": "as updated" },
        });
        if !skip_archive.is_null() {
            update["SkipArchive"] = skip_archive;
        }
        ok(&router, "UpdateTable", &update).await;

        let page = list_versions(&router, "archive_db", None, None).await;
        assert_eq!(
            version_ids(&page),
            expected,
            "case {case}: expected the versions {expected:?} the catalog still holds, got {page}"
        );
    }
}

/// A listing of a table the catalog does not hold, of a database it does not hold, or of
/// another account's catalog is refused with HTTP 400 and the modelled error for that
/// refusal.
#[tokio::test]
async fn an_unlistable_history_is_refused() {
    let router = emulator();
    write_history(
        &router,
        "refusing_db",
        &[json!({ "Name": TABLE, "Description": "as created" })],
    )
    .await;

    // (case, request, expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a table the database does not hold",
            json!({ "DatabaseName": "refusing_db", "TableName": "absent" }),
            "EntityNotFoundException",
        ),
        (
            "a database the catalog does not hold",
            json!({ "DatabaseName": "absent_db", "TableName": TABLE }),
            "EntityNotFoundException",
        ),
        (
            "a catalog of another account, which no resource policy grants",
            json!({
                "CatalogId": "999999999999",
                "DatabaseName": "refusing_db",
                "TableName": TABLE,
            }),
            "AccessDeniedException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "GetTableVersions", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused listing is HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(expected),
            "case {case}: expected {expected}, got {}",
            error_code(&body)
        );
    }

    // The table the catalog DOES hold is still listed after every refusal.
    let page = list_versions(&router, "refusing_db", None, None).await;
    assert_eq!(
        version_ids(&page),
        vec!["0".to_string()],
        "case after the refusals: the stored history is still served, got {page}"
    );
}

/// A version listing is a READ: repeating it, with and without paging, leaves the table,
/// its current version and its history exactly as the writes left them.
#[tokio::test]
async fn listing_versions_changes_nothing() {
    let router = emulator();
    let history = paging_history();
    write_history(&router, "stable_db", &history).await;

    let table_before = get_table(&router, "stable_db").await;
    let listing_before = list_versions(&router, "stable_db", None, None).await;
    let current_before = get_table_version(&router, "stable_db", None).await;

    for _ in 0..2 {
        walk_pages(&router, "stable_db", Some(2)).await;
        list_versions(&router, "stable_db", None, None).await;
        // Including the refused listings, which must write nothing either.
        call(
            &router,
            "GetTableVersions",
            &json!({ "DatabaseName": "stable_db", "TableName": "absent" }),
        )
        .await;
    }

    assert_eq!(
        get_table(&router, "stable_db").await,
        table_before,
        "case after repeated listings: the live table must be untouched, expected {table_before}"
    );
    assert_eq!(
        list_versions(&router, "stable_db", None, None).await,
        listing_before,
        "case after repeated listings: the history must read back the same, expected \
         {listing_before}"
    );
    assert_eq!(
        get_table_version(&router, "stable_db", None).await,
        current_before,
        "case after repeated listings: the current version must be unmoved, expected \
         {current_before}"
    );
}
