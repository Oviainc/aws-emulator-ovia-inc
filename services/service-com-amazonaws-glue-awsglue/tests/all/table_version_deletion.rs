// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteTableVersion`: dropping ONE version out of a table's
//! version history.
//!
//! The rule under test: a version delete removes exactly the archived version its
//! `VersionId` names, so that version stops being readable and listable while the table,
//! its current version and every other version of it stay as the writes left them. The
//! table's NEWEST version is its live definition, so a request addressing it is refused
//! with `InvalidInputException` and changes nothing; a `VersionId` the history does not
//! hold (including one already deleted) is `EntityNotFoundException`, and one that spells
//! no counter is `InvalidInputException`.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness, dispatched by the same `MultiServiceRouter` the production
//! front door uses.
//!
//! The suites are tables: a row is one history (how many writes the table has had and
//! which member set each one submitted) paired with the version deleted out of it, or one
//! undeletable request. The recorded cases — a delete of an earlier version read back
//! through the history, and the refusal of a delete addressing the current version — are
//! rows of the first two suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::table_versions::{get_table, get_table_version, histories, write_history, TABLE};
use crate::wire::{call, emulator, error_code, error_message, ok};

/// The refusal a delete of a table's CURRENT version earns, as the service words it.
///
/// `pub(crate)` because the BATCH version delete reports the same refusal per item
/// ([`crate::batch_table_version_deletion`]), so both suites read one spelling of it.
pub(crate) const LATEST_REFUSAL: &str = "VersionId cannot be equal to the latest.";

/// The refusal a read or a delete of a version the table's history no longer holds earns —
/// which is what a version the table MINTED becomes once it has been deleted. An id the
/// table never minted earns the other version-miss sentence, which `not_found_wording.rs`
/// pins beside this one. `pub(crate)` for the same reason as [`LATEST_REFUSAL`].
pub(crate) const MISSING_VERSION_REFUSAL: &str = "Table version not found.";

/// One `DeleteTableVersion` call: its HTTP status and body.
async fn delete_version(
    router: &MultiServiceRouter,
    database: &str,
    version_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteTableVersion",
        &json!({
            "DatabaseName": database,
            "TableName": TABLE,
            "VersionId": version_id,
        }),
    )
    .await
}

/// The `VersionId`s `GetTableVersions` reports for the table, in the order served.
///
/// `pub(crate)` because the BATCH version delete suite reads the surviving history back
/// through the same listing ([`crate::batch_table_version_deletion`]).
pub(crate) async fn listed_versions(
    router: &MultiServiceRouter,
    database: &str,
) -> Vec<String> {
    let page = ok(
        router,
        "GetTableVersions",
        &json!({ "DatabaseName": database, "TableName": TABLE }),
    )
    .await;
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

/// The `TableVersion` body for every counter in `counters`, read one by one.
///
/// `pub(crate)` for the batch suite's sake, like [`listed_versions`].
pub(crate) async fn versions_of(
    router: &MultiServiceRouter,
    database: &str,
    counters: &[usize],
) -> Vec<(String, Value)> {
    let mut read = Vec::new();
    for counter in counters {
        let version_id = counter.to_string();
        let body = get_table_version(router, database, Some(&version_id)).await;
        read.push((version_id, body));
    }
    read
}

/// Deleting an archived version removes that version and nothing else: it stops being
/// readable and listable, while the live table, the current version and every surviving
/// version read back exactly as before. Each history is tested with EVERY archived
/// version it holds deleted in turn, so the row varies the history's length, the member
/// set each write submitted and which version is dropped.
#[tokio::test]
async fn deleting_an_archived_version_removes_only_that_version() {
    for (case, database, history) in histories() {
        // The last definition is the live one, so only the earlier ones are archived.
        for target in 0..history.len().saturating_sub(1) {
            let case = format!("{case}, deleting version {target}");
            let router = emulator();
            write_history(&router, database, &history).await;

            let survivors: Vec<usize> = (0..history.len()).filter(|c| *c != target).collect();
            let before_table = get_table(&router, database).await;
            let before_versions = versions_of(&router, database, &survivors).await;

            let (status, answer) = delete_version(&router, database, &target.to_string()).await;
            assert_eq!(
                status, 200,
                "case {case}: deleting an archived version is served, got {status} with {answer}"
            );

            // The deleted version is gone from the single-version read.
            let (status, answer) = call(
                &router,
                "GetTableVersion",
                &json!({
                    "DatabaseName": database,
                    "TableName": TABLE,
                    "VersionId": target.to_string(),
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: the deleted version is no longer readable, got {status} with \
                 {answer}"
            );
            assert!(
                error_code(&answer).ends_with("EntityNotFoundException"),
                "case {case}: expected EntityNotFoundException, got {}",
                error_code(&answer)
            );
            assert_eq!(
                error_message(&answer),
                MISSING_VERSION_REFUSAL,
                "case {case}: expected the refusal {MISSING_VERSION_REFUSAL:?}, got {:?}",
                error_message(&answer)
            );

            // ... and from the history listing, which still holds every other version.
            let expected: Vec<String> = survivors.iter().map(usize::to_string).collect();
            let listed = listed_versions(&router, database).await;
            assert_eq!(
                listed, expected,
                "case {case}: the listing holds every surviving version and no other; expected \
                 {expected:?}, got {listed:?}"
            );

            // The table itself, and every surviving version, are untouched.
            let after_table = get_table(&router, database).await;
            assert_eq!(
                after_table, before_table,
                "case {case}: the live table must be untouched; expected {before_table}, got \
                 {after_table}"
            );
            let after_versions = versions_of(&router, database, &survivors).await;
            assert_eq!(
                after_versions, before_versions,
                "case {case}: every surviving version must read back the same; expected \
                 {before_versions:?}, got {after_versions:?}"
            );
        }
    }
}

/// The table's current version is its live definition, so a delete addressing it is
/// refused and writes nothing — whatever the table's history, and whichever spelling of
/// the counter the request uses.
#[tokio::test]
async fn deleting_the_current_version_is_refused() {
    for (case, database, history) in histories() {
        let current = history.len() - 1;
        // The canonical spelling of the current counter, and a zero-padded one: the
        // version addressed is the counter the id spells, not the string sent.
        for version_id in [current.to_string(), format!("0{current}")] {
            let case = format!("{case}, deleting the current version as {version_id}");
            let router = emulator();
            write_history(&router, database, &history).await;

            let counters: Vec<usize> = (0..history.len()).collect();
            let before_table = get_table(&router, database).await;
            let before_versions = versions_of(&router, database, &counters).await;

            let (status, answer) = delete_version(&router, database, &version_id).await;
            assert_eq!(
                status, 400,
                "case {case}: the current version is not deletable, got {status} with {answer}"
            );
            assert!(
                error_code(&answer).ends_with("InvalidInputException"),
                "case {case}: expected InvalidInputException, got {}",
                error_code(&answer)
            );
            assert_eq!(
                error_message(&answer),
                LATEST_REFUSAL,
                "case {case}: expected the refusal {LATEST_REFUSAL:?}, got {:?}",
                error_message(&answer)
            );

            // The refusal wrote nothing: the table and its whole history stand.
            let after_table = get_table(&router, database).await;
            assert_eq!(
                after_table, before_table,
                "case {case}: a refused delete must not write; expected {before_table}, got \
                 {after_table}"
            );
            let after_versions = versions_of(&router, database, &counters).await;
            assert_eq!(
                after_versions, before_versions,
                "case {case}: a refused delete must leave every version readable; expected \
                 {before_versions:?}, got {after_versions:?}"
            );
            let listed = listed_versions(&router, database).await;
            let expected: Vec<String> = counters.iter().map(usize::to_string).collect();
            assert_eq!(
                listed, expected,
                "case {case}: a refused delete must leave the history whole; expected \
                 {expected:?}, got {listed:?}"
            );
        }
    }
}

/// A version this operation cannot delete is refused with the error its condition earns:
/// `InvalidInputException` for a `VersionId` that spells no counter, and
/// `EntityNotFoundException` for a version, table or database the catalog does not hold.
/// The rows span both kinds of malformed id and every level of absence.
#[tokio::test]
async fn an_undeletable_version_is_refused() {
    let router = emulator();
    write_history(
        &router,
        "refusing_delete_db",
        &[
            json!({ "Name": TABLE, "Description": "as created" }),
            json!({ "Name": TABLE, "Description": "as updated once" }),
            json!({ "Name": TABLE, "Description": "as updated twice" }),
        ],
    )
    .await;

    // (case, request, expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a version past the newest",
            json!({ "DatabaseName": "refusing_delete_db", "TableName": TABLE, "VersionId": "7" }),
            "EntityNotFoundException",
        ),
        (
            "a version id that names no number",
            json!({
                "DatabaseName": "refusing_delete_db",
                "TableName": TABLE,
                "VersionId": "oldest",
            }),
            "InvalidInputException",
        ),
        (
            "a version id that names no integer",
            json!({ "DatabaseName": "refusing_delete_db", "TableName": TABLE, "VersionId": "1.5" }),
            "InvalidInputException",
        ),
        (
            "a version id that names no counter",
            json!({ "DatabaseName": "refusing_delete_db", "TableName": TABLE, "VersionId": "-1" }),
            "InvalidInputException",
        ),
        (
            "a table the database does not hold",
            json!({ "DatabaseName": "refusing_delete_db", "TableName": "absent", "VersionId": "0" }),
            "EntityNotFoundException",
        ),
        (
            "a database the catalog does not hold",
            json!({ "DatabaseName": "absent_db", "TableName": TABLE, "VersionId": "0" }),
            "EntityNotFoundException",
        ),
        (
            "a catalog of another account, which no resource policy grants",
            json!({
                "CatalogId": "999999999999",
                "DatabaseName": "refusing_delete_db",
                "TableName": TABLE,
                "VersionId": "0",
            }),
            "AccessDeniedException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, answer) = call(&router, "DeleteTableVersion", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused version delete is HTTP 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).ends_with(expected),
            "case {case}: expected {expected}, got {}",
            error_code(&answer)
        );
    }

    // Every refusal left the history whole, so the archived versions are still deletable
    // — and a version already deleted has become a miss, in the padded spelling too.
    for (case, version_id) in [("the first version", "0"), ("a padded second version", "01")] {
        let (status, answer) = delete_version(&router, "refusing_delete_db", version_id).await;
        assert_eq!(
            status, 200,
            "case {case}: an archived version is deletable after the refusals, got {status} with \
             {answer}"
        );
        let (status, answer) = delete_version(&router, "refusing_delete_db", version_id).await;
        assert_eq!(
            status, 400,
            "case {case}: deleting it a second time is a miss, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {}",
            error_code(&answer)
        );
        assert_eq!(
            error_message(&answer),
            MISSING_VERSION_REFUSAL,
            "case {case}: expected the refusal {MISSING_VERSION_REFUSAL:?}, got {:?}",
            error_message(&answer)
        );
    }

    // What is left is the table's current version, which stays: the table is still there.
    let listed = listed_versions(&router, "refusing_delete_db").await;
    assert_eq!(
        listed,
        vec!["2".to_string()],
        "case after deleting both archived versions: only the current version is left, got \
         {listed:?}"
    );
    let table = get_table(&router, "refusing_delete_db").await;
    assert_eq!(
        table["Description"].as_str(),
        Some("as updated twice"),
        "case after deleting both archived versions: the table itself is untouched, got {table}"
    );
}
